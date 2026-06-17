//! Read and write PSS/E `.raw` (revisions 33-35; see [`write_psse_rev`]).
//!
//! Covers the core sections — bus, load, fixed shunt, generator, branch, and the
//! 2- and 3-winding transformer records — which together carry a transmission
//! power flow case. A switched shunt keeps its steady-state susceptance `BINIT`
//! as the shunt `b` and carries its mode, voltage band, regulated bus, RMPCT, and
//! step blocks on [`SwitchedShuntControl`]. Impedances are written on the system base with
//! per-unit turns ratios (`CZ = 1`, `CW = 1`); the reader assumes the same and
//! does not convert other impedance/turns bases — a non-unit `CZ`/`CW` is read
//! verbatim (so misread). Two-terminal DC lines read and write as the neutral
//! [`Hvdc`] (power-setpoint model; converter firing-angle/transformer detail
//! rides through in extras). The other advanced sections (VSC and multi-terminal
//! DC, FACTS, GNE) are not modeled: on write they're emitted as empty sections,
//! on read they're skipped, and storage carried on the `Network` is reported as
//! dropped. Same-format round-trip is byte-exact via the retained source (see
//! [`crate::write_as`]); this serializer is the cross-format path.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;

use serde_json::Value;

use super::{Conversion, jnum, sanitize_quoted};
use crate::network::{
    Area, Branch, Bus, BusId, BusType, Extras, Generator, Hvdc, Impedance, Load, Network, Shunt,
    ShuntBlock, SolverParams, SourceFormat, SwitchedShuntControl, SwitchedShuntMode, Transformer3W,
    TransformerControl, TransformerControlMode, Winding,
};
use crate::{Error, Result};

const FMT: &str = "PSS/E .raw";
const REV: u32 = 33;

/// Characters that would corrupt a single-quoted PSS/E name field. The quote
/// toggles the reader's quoted state early, and `/` truncates the record at the
/// inline-comment delimiter (a PSS/E record splits on `/` before tokenizing).
const NAME_FORBIDDEN: &[char] = &['\'', '/'];

// ---- Writer -----------------------------------------------------------------

/// Serialize `net` to PSS/E `.raw` at the default revision (33).
#[must_use]
pub fn write_psse(net: &Network) -> Conversion {
    write_psse_rev(net, REV)
}

/// Serialize `net` to PSS/E `.raw` at `rev` (33, 34, or 35).
///
/// Revisions 34 and 35 add the expanded system-wide header with its
/// end-of-system-wide-data marker, the named 12-rating branch record (the reader
/// keys its branch layout off the header revision), and the load
/// distributed-generation / load-type trailing columns. Any other `rev` falls
/// back to the 33 layout. Same-format byte-exact echo still rides the retained
/// source (see [`crate::write_as`]); this serializer is the cross-format path.
#[must_use]
// A flat serializer: one stanza per PSS/E record type; splitting it would add
// indirection without clarity.
#[expect(clippy::too_many_lines)]
pub fn write_psse_rev(net: &Network, rev: u32) -> Conversion {
    // v34+ wraps the global parameters in a system-wide data section, names
    // branches and carries 12 ratings, and adds load DG / load-type columns.
    let modern = rev >= 34;
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
        "0, {}, {rev}, 0, {}, {}   / powerio export: {}",
        net.base_mva,
        i32::from(modern),
        num(net.base_frequency),
        net.name
    );
    let _ = writeln!(s, "{}", net.name);
    let _ = writeln!(s);
    if modern {
        // v34+ system-wide block: emit the solver keyword lines (the fields that
        // are set), then close the block.
        if let Some(sp) = &net.solver {
            if let Some(t) = sp.zero_impedance_threshold {
                let _ = writeln!(s, "GENERAL, THRSHZ={}", num(t));
            }
            let mut newton = Vec::new();
            if let Some(t) = sp.newton_tolerance {
                newton.push(format!("TOLN={}", num(t)));
            }
            if let Some(n) = sp.max_iterations {
                newton.push(format!("ITMXN={n}"));
            }
            if !newton.is_empty() {
                let _ = writeln!(s, "NEWTON, {}", newton.join(", "));
            }
            let flags: Vec<String> = [
                ("ACTAPS", sp.adjust_taps),
                ("AREAIN", sp.adjust_area_interchange),
                ("PHSHFT", sp.adjust_phase_shift),
                ("DCTAPS", sp.adjust_dc_taps),
                ("SWSHNT", sp.adjust_switched_shunt),
            ]
            .into_iter()
            .filter_map(|(name, v)| v.map(|b| format!("{name}={}", i32::from(b))))
            .collect();
            if !flags.is_empty() {
                let _ = writeln!(s, "SOLVER, {}", flags.join(", "));
            }
        }
        let _ = writeln!(s, "0 / END OF SYSTEM-WIDE DATA, BEGIN BUS DATA");
    }

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

    // v33 ends the load record at INTRPT; v34 adds PDGEN/QDGEN/STDG and v35 a
    // LOADTYPE string. powerio's load carries none of these, so they trail as
    // defaults; the reader reads PL/QL by fixed index and ignores the rest.
    let load_tail = if rev >= 35 {
        ", 0, 0, 0, ''"
    } else if modern {
        ", 0, 0, 0"
    } else {
        ""
    };
    for l in &net.loads {
        let (area, zone) = bus_area.get(&l.bus).copied().unwrap_or((1, 1));
        let _ = writeln!(
            s,
            "{}, '1', {}, {}, {}, {}, {}, 0, 0, 0, 0, 1, 1, 0{load_tail}",
            l.bus,
            i32::from(l.in_service),
            area,
            zone,
            num(l.p),
            num(l.q)
        );
    }
    let _ = writeln!(s, "0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA");

    // Fixed shunts here; switched shunts (control = Some) go in their own section.
    for sh in net.shunts.iter().filter(|s| s.control.is_none()) {
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
        if modern {
            // v34+: a quoted line NAME at field 6, then twelve rating columns,
            // pushing STAT to field 23 (the layout the reader expects at rev>=34).
            // ratings 4-12 default to 0 (powerio carries only rate_a/b/c).
            let _ = writeln!(
                s,
                "{}, {}, '1', {}, {}, {}, '            ', {}, {}, {}, \
                 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, {}, 1, 0, 1, 1",
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
        } else {
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
        // Winding-1 control columns (COD, CONT, RMA/RMI, VMA/VMI, NTP) come from
        // the regulating-control data when present, else the fixed defaults.
        let ctl = br.control.as_ref();
        let sbase = ctl
            .filter(|c| c.mva_base > 0.0)
            .map_or(net.base_mva, |c| c.mva_base);
        let cod = ctl.map_or(0, |c| mode_to_cod(c.mode));
        let cont = ctl.and_then(|c| c.controlled_bus).map_or(0, |b| b.0);
        let (rma, rmi, vma, vmi, ntp) = ctl.map_or((1.1, 0.9, 1.1, 0.9, 33), |c| {
            (c.tap_max, c.tap_min, c.band_max, c.band_min, c.ntp)
        });
        let _ = writeln!(s, "{}, {}, {}", num(br.r), num(br.x), num(sbase));
        let _ = writeln!(
            s,
            "{}, 0, {}, {}, {}, {}, {cod}, {cont}, {}, {}, {}, {}, {ntp}, 0, 0, 0, 0",
            num(br.effective_tap()),
            num(br.shift),
            num(br.rate_a),
            num(br.rate_b),
            num(br.rate_c),
            num(rma),
            num(rmi),
            num(vma),
            num(vmi)
        );
        let _ = writeln!(s, "1.0, 0");
    }

    // 3-winding transformers: a 5-line record. CW=1, CZ=1, CM=1 (same conventions
    // as the 2-winding record); line 2 carries the three pairwise impedances and
    // the star-point voltage, lines 3-5 the per-winding tap/angle/ratings.
    for t in &net.transformers_3w {
        let raw_name = t.name.as_deref().unwrap_or("");
        let name = sanitize_quoted(raw_name, NAME_FORBIDDEN, ' ');
        if matches!(name, std::borrow::Cow::Owned(_)) {
            sanitized_names += 1;
        }
        let _ = writeln!(
            s,
            "{}, {}, {}, '1', 1, 1, 1, {}, {}, 2, '{:<12}', {}, 1, 1, 0, 1, 0, 1, 0, 1, '            '",
            t.windings[0].bus,
            t.windings[1].bus,
            t.windings[2].bus,
            num(t.mag_g),
            num(t.mag_b),
            name,
            i32::from(t.in_service)
        );
        // Line 2: the three pairwise (R, X) on the system base (CZ=1), each with
        // its declared SBASE column, then the star voltage.
        let [z12, z23, z31] = t.z;
        let _ = writeln!(
            s,
            "{}, {}, {}, {}, {}, {}, {}, {}, {}, {}, {}",
            num(z12.r),
            num(z12.x),
            num(z12.base_mva),
            num(z23.r),
            num(z23.x),
            num(z23.base_mva),
            num(z31.r),
            num(z31.x),
            num(z31.base_mva),
            num(t.star_vm),
            num(t.star_va)
        );
        for w in &t.windings {
            let _ = writeln!(
                s,
                "{}, {}, {}, {}, {}, {}, 0, 0, 1.1, 0.9, 1.1, 0.9, 33, 0, 0, 0, 0",
                num(w.tap),
                num(w.nominal_kv),
                num(w.shift),
                num(w.rate_a),
                num(w.rate_b),
                num(w.rate_c)
            );
        }
    }
    let _ = writeln!(s, "0 / END OF TRANSFORMER DATA, BEGIN AREA DATA");
    for a in &net.areas {
        let raw_name = a.name.as_deref().unwrap_or("");
        let name = sanitize_quoted(raw_name, NAME_FORBIDDEN, ' ');
        if matches!(name, std::borrow::Cow::Owned(_)) {
            sanitized_names += 1;
        }
        let _ = writeln!(
            s,
            "{}, {}, {}, {}, '{:<12}'",
            a.number,
            a.slack_bus.map_or(0, |b| b.0),
            num(a.net_interchange),
            num(a.tolerance),
            name
        );
    }

    // Two-terminal DC lines occupy the first of the otherwise-empty sections:
    // emit their 3-line records (if any) between the begin/end markers, then the
    // remaining sections as bare terminators so the file parses as a complete case.
    let _ = writeln!(s, "{}", EMPTY_SECTIONS[0]);
    for (i, dc) in net.hvdc.iter().enumerate() {
        let name = format!(
            "'{}'",
            dc_str(&dc.extras, "psse_dc_name").unwrap_or_else(|| format!("DC{}", i + 1))
        );
        let mdc = if dc.in_service {
            dc_int(&dc.extras, "psse_dc_mdc").unwrap_or(1)
        } else {
            0
        };
        let rdc = dc_f64(&dc.extras, "psse_dc_rdc").unwrap_or(0.0);
        let vschd = dc_f64(&dc.extras, "psse_dc_vschd").unwrap_or(0.0);
        let l1_tail = dc_tail(
            &dc.extras,
            "psse_dc_control_tail",
            "0.0, 0.0, 0.0, 'I', 0.0, 20, 1.0",
        );
        let rect_tail = dc_tail(&dc.extras, "psse_dc_rectifier_tail", DEFAULT_CONVERTER_TAIL);
        let inv_tail = dc_tail(&dc.extras, "psse_dc_inverter_tail", DEFAULT_CONVERTER_TAIL);
        let _ = writeln!(
            s,
            "{name}, {mdc}, {}, {}, {}, {l1_tail}",
            num(rdc),
            num(dc.pf),
            num(vschd)
        );
        let _ = writeln!(s, "{}, {rect_tail}", dc.from);
        let _ = writeln!(s, "{}, {inv_tail}", dc.to);
    }
    // Sections up to and including the SWITCHED SHUNT begin marker.
    for line in &EMPTY_SECTIONS[1..=9] {
        let _ = writeln!(s, "{line}");
    }
    // Switched shunts: BINIT becomes the susceptance, the control record the rest.
    for sh in net.shunts.iter().filter(|s| s.control.is_some()) {
        let Some(c) = sh.control.as_ref() else {
            continue;
        };
        let swrem = c.control_bus.map_or(0, |b| b.0);
        let mut blocks = String::new();
        for blk in &c.blocks {
            let _ = write!(blocks, ", {}, {}", blk.steps, num(blk.b));
        }
        let _ = writeln!(
            s,
            "{}, {}, 0, {}, {}, {}, {swrem}, {}, '', {}{blocks}",
            sh.bus,
            mode_to_modsw(c.mode),
            i32::from(sh.in_service),
            num(c.vhigh),
            num(c.vlow),
            num(c.rmpct),
            num(sh.b)
        );
    }
    for line in &EMPTY_SECTIONS[10..] {
        let _ = writeln!(s, "{line}");
    }
    let _ = writeln!(s, "Q");

    if net
        .hvdc
        .iter()
        .any(|d| !d.extras.contains_key("psse_dc_name"))
    {
        warnings.push(
            "DC line converter detail (firing angles, converter transformer taps, reactive \
             output) defaulted: PSS/E two-terminal DC is written from the power setpoint and \
             line resistance only"
                .into(),
        );
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

/// Converter-line tail (everything after the AC terminal bus) for a synthesized
/// two-terminal DC record: NBR/NBI bridges, firing-angle limits, converter
/// transformer R/X and tap data, and the metered-end id. PSS/E-sourced lines
/// replay their own tail; these defaults serve a cross-format source.
const DEFAULT_CONVERTER_TAIL: &str =
    "1, 15.0, 5.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.5, 0.51, 0.00625, 0, 0, 0, '1', 0.0";

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

/// Parse a PSS/E `.raw` (revisions 33-35) into a [`Network`]. Reads bus/load/
/// fixed-shunt/generator/branch/2- and 3-winding transformer; skips the advanced
/// sections.
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
    let mut transformers_3w = Vec::new();
    let mut hvdc = Vec::new();
    let mut areas = Vec::new();
    let mut solver = SolverParams::default();

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
                // The v34+ system-wide block precedes the bus data; capture its
                // solver keyword lines (this is the first one that triggered).
                section = Section::SystemWide;
                parse_solver_line(&f, &mut solver);
            }
            Section::Bus => buses.push(read_bus(&f)?),
            Section::Load => loads.push(read_load(&f)?),
            Section::FixedShunt => shunts.push(read_shunt(&f)?),
            Section::SwitchedShunt => shunts.push(read_switched_shunt(&f)?),
            Section::Generator => generators.push(read_gen(&f)?),
            Section::Branch => branches.push(read_branch(&f, raw_rev)?),
            Section::Transformer => {
                // 2-winding = 4 lines (K field == 0); 3-winding = 5 lines.
                let two_winding = f.get(2).and_then(|x| x.parse::<i64>().ok()) == Some(0);
                let l2 = lines.next().map_or("", str::trim);
                let l3 = lines.next().map_or("", str::trim);
                let l4 = lines.next().map_or("", str::trim);
                if two_winding {
                    branches.push(read_transformer(&f, &fields(l2), &fields(l3), &fields(l4))?);
                } else {
                    let l5 = lines.next().map_or("", str::trim);
                    transformers_3w.push(read_transformer_3w(
                        &f,
                        &fields(l2),
                        &fields(l3),
                        &fields(l4),
                        &fields(l5),
                    )?);
                }
            }
            Section::TwoTerminalDc => {
                // 3-line record: control line, then the rectifier and inverter
                // converter lines whose first field is the AC terminal bus.
                let rectifier = lines.next().map_or("", str::trim);
                let inverter = lines.next().map_or("", str::trim);
                hvdc.push(read_dc_line(&f, &fields(rectifier), &fields(inverter))?);
            }
            Section::Area => areas.push(read_area(&f)?),
            Section::SystemWide => parse_solver_line(&f, &mut solver),
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
        hvdc,
        transformers_3w,
        areas,
        solver: (!solver.is_empty()).then_some(solver),
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
    TwoTerminalDc,
    Area,
    SystemWide,
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
    } else if u.contains("BEGIN TWO-TERMINAL DC DATA") {
        Section::TwoTerminalDc
    } else if u.contains("BEGIN AREA DATA") {
        // Distinct from "BEGIN INTER-AREA TRANSFER DATA", which doesn't contain
        // the exact "BEGIN AREA DATA" run.
        Section::Area
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
        Some(first) if matches!(first.as_str(), "GENERAL" | "RATING" | "NEWTON" | "SOLVER")
    )
}

/// Parse a v34+ system-wide keyword line (`GENERAL`/`NEWTON`/`SOLVER`, each a
/// keyword then `KEY=VALUE` tokens) into the solver record. Unrecognized
/// keywords (e.g. `RATING`) and keys are ignored.
fn parse_solver_line(f: &[String], solver: &mut SolverParams) {
    let Some(keyword) = f.first().map(|s| s.to_ascii_uppercase()) else {
        return;
    };
    for tok in &f[1..] {
        let Some((key, val)) = tok.split_once('=') else {
            continue;
        };
        let (key, val) = (key.trim().to_ascii_uppercase(), val.trim());
        match (keyword.as_str(), key.as_str()) {
            ("GENERAL", "THRSHZ") => solver.zero_impedance_threshold = val.parse().ok(),
            ("NEWTON", "TOLN") => solver.newton_tolerance = val.parse().ok(),
            ("NEWTON", "ITMXN") => solver.max_iterations = val.parse().ok(),
            ("SOLVER", "ACTAPS") => solver.adjust_taps = Some(parse_enable(val)),
            ("SOLVER", "AREAIN") => solver.adjust_area_interchange = Some(parse_enable(val)),
            ("SOLVER", "PHSHFT") => solver.adjust_phase_shift = Some(parse_enable(val)),
            ("SOLVER", "DCTAPS") => solver.adjust_dc_taps = Some(parse_enable(val)),
            ("SOLVER", "SWSHNT") => solver.adjust_switched_shunt = Some(parse_enable(val)),
            _ => {}
        }
    }
}

/// A `SOLVER` adjustment flag: numeric → nonzero is enabled; a keyword is enabled
/// unless it reads as off.
fn parse_enable(val: &str) -> bool {
    val.parse::<f64>().map_or_else(
        |_| !matches!(val.to_ascii_uppercase().as_str(), "DISABLED" | "OFF" | "NO"),
        |n| n != 0.0,
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
        control: None,
        extras: Extras::new(),
    })
}

fn read_switched_shunt(f: &[String]) -> Result<Shunt> {
    // I, MODSW, ADJM, STAT, VSWHI, VSWLO, SWREM, RMPCT, RMIDNT, BINIT(9), N1, B1, ...
    // The steady-state susceptance BINIT becomes the shunt `b` (gs = 0); the
    // mode, voltage band, regulated bus, RMPCT, and the (Ni, Bi) step blocks ride
    // on the switching-control record.
    let bus = id_at(f, 0, 0)?;
    let swrem = id_at(f, 6, 0)?;
    // Step blocks are (count, susceptance) pairs from field 10; stop at the first
    // empty (padding) block or the end of the record.
    let mut blocks = Vec::new();
    let mut i = 10;
    while i + 1 < f.len() {
        let steps = int_at(f, i, 0)?;
        let b = num_at(f, i + 1, 0.0)?;
        if steps == 0 && b == 0.0 {
            break;
        }
        blocks.push(ShuntBlock {
            steps: steps.clamp(0, i64::from(u32::MAX)) as u32,
            b,
        });
        i += 2;
    }
    let control = SwitchedShuntControl {
        mode: modsw_to_mode(int_at(f, 1, 1)?),
        vhigh: num_at(f, 4, 0.0)?,
        vlow: num_at(f, 5, 0.0)?,
        control_bus: (swrem != 0 && swrem != bus).then_some(BusId(swrem)),
        rmpct: num_at(f, 7, 100.0)?,
        blocks,
    };
    Ok(Shunt {
        bus: BusId(bus),
        g: 0.0,
        b: num_at(f, 9, 0.0)?,
        in_service: on_at(f, 3, true)?,
        control: Some(control),
        extras: Extras::new(),
    })
}

/// PSS/E `MODSW` switched-shunt mode code → neutral mode.
fn modsw_to_mode(modsw: i64) -> SwitchedShuntMode {
    match modsw {
        0 => SwitchedShuntMode::Locked,
        1 => SwitchedShuntMode::Continuous,
        _ => SwitchedShuntMode::Discrete,
    }
}

/// Neutral switched-shunt mode → PSS/E `MODSW` (the 0/1/2 codes; modes beyond
/// discrete collapse to 2).
fn mode_to_modsw(mode: SwitchedShuntMode) -> i64 {
    match mode {
        SwitchedShuntMode::Locked => 0,
        SwitchedShuntMode::Continuous => 1,
        SwitchedShuntMode::Discrete => 2,
    }
}

fn read_area(f: &[String]) -> Result<Area> {
    // I, ISW, PDES, PTOL, 'ARNAME'
    let isw = id_at(f, 1, 0)?;
    Ok(Area {
        number: id_at(f, 0, 0)?,
        slack_bus: (isw != 0).then_some(BusId(isw)),
        net_interchange: num_at(f, 2, 0.0)?,
        tolerance: num_at(f, 3, 0.0)?,
        name: f
            .get(4)
            .filter(|n| !n.trim().is_empty())
            .map(|n| n.trim().to_string()),
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
        control: None,
        extras: Extras::new(),
    })
}

fn read_transformer(l1: &[String], l2: &[String], l3: &[String], _l4: &[String]) -> Result<Branch> {
    // l1: I, J, K, CKT, CW, CZ, CM, MAG1, MAG2, NMETR, NAME, STAT(11)
    // l2: R1-2, X1-2, SBASE1-2
    // l3: WINDV1, NOMV1, ANG1, RATA1, RATB1, RATC1, COD1, CONT1, RMA1, RMI1,
    //     VMA1, VMI1, NTP1, ...
    // A nonzero control code COD1 marks a regulating winding; capture its limits
    // and regulated bus, else leave the branch's control unset.
    let cod = int_at(l3, 6, 0)?;
    let control = (cod != 0)
        .then(|| -> Result<TransformerControl> {
            let cont = id_at(l3, 7, 0)?;
            Ok(TransformerControl {
                mode: cod_to_mode(cod),
                controlled_bus: (cont != 0).then_some(BusId(cont)),
                tap_max: num_at(l3, 8, 1.1)?,
                tap_min: num_at(l3, 9, 0.9)?,
                band_max: num_at(l3, 10, 1.1)?,
                band_min: num_at(l3, 11, 0.9)?,
                ntp: int_at(l3, 12, 33)?.clamp(0, i64::from(u32::MAX)) as u32,
                mva_base: num_at(l2, 2, 0.0)?,
            })
        })
        .transpose()?;
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
        control,
        extras: Extras::new(),
    })
}

/// PSS/E transformer control code `COD` → neutral control mode. The sign encodes
/// an enable/disable flag PSS/E carries separately; only the magnitude selects
/// the regulation kind.
fn cod_to_mode(cod: i64) -> TransformerControlMode {
    match cod.abs() {
        1 => TransformerControlMode::Voltage,
        2 => TransformerControlMode::ReactiveFlow,
        3 => TransformerControlMode::ActiveFlow,
        _ => TransformerControlMode::Fixed,
    }
}

/// Neutral control mode → PSS/E `COD` (positive; the enable-flag sign is not modeled).
fn mode_to_cod(mode: TransformerControlMode) -> i64 {
    match mode {
        TransformerControlMode::Fixed => 0,
        TransformerControlMode::Voltage => 1,
        TransformerControlMode::ReactiveFlow => 2,
        TransformerControlMode::ActiveFlow => 3,
    }
}

/// Read a 5-line 3-winding transformer record into a [`Transformer3W`].
///
/// As with the 2-winding reader, `CZ = 1` is assumed, so the pairwise R/X are
/// taken on the system base verbatim (a non-unit `CZ` is misread — the same
/// limitation the 2-winding path has).
fn read_transformer_3w(
    l1: &[String],
    l2: &[String],
    l3: &[String],
    l4: &[String],
    l5: &[String],
) -> Result<Transformer3W> {
    // l1: I, J, K, CKT, CW, CZ, CM, MAG1, MAG2, NMETR, NAME, STAT(11)
    // l2: R1-2,X1-2,SBASE1-2, R2-3,X2-3,SBASE2-3, R3-1,X3-1,SBASE3-1, VMSTAR, ANSTAR
    // l3/l4/l5: WINDVk, NOMVk, ANGk, RATAk, RATBk, RATCk, ...
    // (R, X, SBASE) for a winding pair; at CZ = 1 the impedance is already on the
    // system base, so the SBASE column is carried only to write it back.
    let imp = |off: usize| -> Result<Impedance> {
        Ok(Impedance {
            r: num_at(l2, off, 0.0)?,
            x: num_at(l2, off + 1, 0.0)?,
            base_mva: num_at(l2, off + 2, 0.0)?,
        })
    };
    let winding = |bus_field: usize, w: &[String]| -> Result<Winding> {
        Ok(Winding {
            bus: BusId(id_at(l1, bus_field, 0)?),
            tap: num_at(w, 0, 1.0)?,
            shift: num_at(w, 2, 0.0)?,
            nominal_kv: num_at(w, 1, 0.0)?,
            rate_a: num_at(w, 3, 0.0)?,
            rate_b: num_at(w, 4, 0.0)?,
            rate_c: num_at(w, 5, 0.0)?,
        })
    };
    Ok(Transformer3W {
        windings: [winding(0, l3)?, winding(1, l4)?, winding(2, l5)?],
        z: [imp(0)?, imp(3)?, imp(6)?],
        star_vm: num_at(l2, 9, 1.0)?,
        star_va: num_at(l2, 10, 0.0)?,
        mag_g: num_at(l1, 7, 0.0)?,
        mag_b: num_at(l1, 8, 0.0)?,
        // STAT 0 = out of service; 1-4 mark which windings are in service. Treat
        // any nonzero status as the transformer being in service.
        in_service: int_at(l1, 11, 1)? != 0,
        name: l1
            .get(10)
            .filter(|n| !n.is_empty())
            .map(|n| n.trim().to_string()),
        extras: Extras::new(),
    })
}

/// Read a 3-line two-terminal DC line record into an [`Hvdc`].
///
/// The control line `l1` gives the operating mode (`MDC`), the DC line resistance
/// (`RDC`), the power/current demand (`SETVL`), and the scheduled DC voltage
/// (`VSCHD`). The rectifier and inverter lines' first field is the AC terminal
/// bus, which becomes the HVDC from/to. The HVDC is read as a power-setpoint
/// model (`pf = pt = SETVL`, no reactive output); the converter detail beyond the
/// buses (firing angles, converter transformer taps) is retained in extras for a
/// faithful write-back, not modeled electrically.
fn read_dc_line(l1: &[String], rect: &[String], inv: &[String]) -> Result<Hvdc> {
    let mdc = int_at(l1, 1, 1)?;
    let rdc = num_at(l1, 2, 0.0)?;
    let setvl = num_at(l1, 3, 0.0)?;
    let vschd = num_at(l1, 4, 0.0)?;
    let mut extras = Extras::new();
    if let Some(name) = l1.first().filter(|n| !n.is_empty()) {
        extras.insert("psse_dc_name".into(), Value::String(name.clone()));
    }
    extras.insert("psse_dc_mdc".into(), Value::from(mdc));
    extras.insert("psse_dc_rdc".into(), jnum(rdc));
    extras.insert("psse_dc_vschd".into(), jnum(vschd));
    extras.insert("psse_dc_control_tail".into(), tail_array(l1, 5));
    extras.insert("psse_dc_rectifier_tail".into(), tail_array(rect, 1));
    extras.insert("psse_dc_inverter_tail".into(), tail_array(inv, 1));
    Ok(Hvdc {
        from: BusId(id_at(rect, 0, 0)?),
        to: BusId(id_at(inv, 0, 0)?),
        in_service: mdc != 0,
        pf: setvl,
        pt: setvl,
        qf: 0.0,
        qt: 0.0,
        vf: 1.0,
        vt: 1.0,
        pmin: 0.0,
        pmax: setvl.abs(),
        qminf: 0.0,
        qmaxf: 0.0,
        qmint: 0.0,
        qmaxt: 0.0,
        loss0: 0.0,
        loss1: 0.0,
        extras,
    })
}

/// The fields of `f` from index `start` as a JSON string array (for extras).
fn tail_array(f: &[String], start: usize) -> Value {
    Value::Array(
        f.iter()
            .skip(start)
            .map(|s| Value::String(s.clone()))
            .collect(),
    )
}

/// A string-valued DC extra.
fn dc_str(extras: &Extras, key: &str) -> Option<String> {
    extras.get(key).and_then(Value::as_str).map(str::to_owned)
}

/// An integer-valued DC extra.
fn dc_int(extras: &Extras, key: &str) -> Option<i64> {
    extras.get(key).and_then(Value::as_i64)
}

/// A float-valued DC extra.
fn dc_f64(extras: &Extras, key: &str) -> Option<f64> {
    extras.get(key).and_then(Value::as_f64)
}

/// A retained converter-line tail joined back into a record fragment, or
/// `default` when the element carries none (a cross-format source).
fn dc_tail(extras: &Extras, key: &str, default: &str) -> String {
    match extras.get(key).and_then(Value::as_array) {
        Some(arr) if !arr.is_empty() => arr
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(", "),
        _ => default.to_string(),
    }
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
    fn reads_and_writes_solver_params() {
        let raw = r"0, 100.00, 34, 0, 1, 60.00 / x
CASE
COMMENT
GENERAL, THRSHZ=0.0001
NEWTON, TOLN=0.1, ITMXN=25
SOLVER, ACTAPS=1, AREAIN=0, PHSHFT=1, DCTAPS=1, SWSHNT=0
0 / END OF SYSTEM-WIDE DATA, BEGIN BUS DATA
1,'B1          ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
Q
";
        let net = parse_psse(raw).unwrap();
        let sp = net.solver.as_ref().expect("solver params parsed");
        close(sp.zero_impedance_threshold.unwrap(), 0.0001);
        close(sp.newton_tolerance.unwrap(), 0.1);
        assert_eq!(sp.max_iterations, Some(25));
        assert_eq!(sp.adjust_taps, Some(true));
        assert_eq!(sp.adjust_area_interchange, Some(false));
        assert_eq!(sp.adjust_phase_shift, Some(true));
        assert_eq!(sp.adjust_switched_shunt, Some(false));

        // Round trip at rev 34 keeps the tolerances and the adjustment flags.
        let net2 = parse_psse(&write_psse_rev(&net, 34).text).unwrap();
        let sp2 = net2
            .solver
            .as_ref()
            .expect("solver params survive the write");
        close(sp2.newton_tolerance.unwrap(), 0.1);
        assert_eq!(sp2.max_iterations, Some(25));
        assert_eq!(sp2.adjust_taps, Some(true));
        assert_eq!(sp2.adjust_area_interchange, Some(false));
    }

    #[test]
    fn reads_and_writes_area_records() {
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / x
CASE
COMMENT
1,'B1          ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
5,'B5          ', 230.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
1, 5, 100.0, 10.0, 'AREA-ONE    '
0 / END OF AREA DATA, BEGIN TWO-TERMINAL DC DATA
Q
";
        let net = parse_psse(raw).unwrap();
        assert_eq!(net.areas.len(), 1, "the area record was read");
        let a = &net.areas[0];
        assert_eq!(a.number, 1);
        assert_eq!(a.slack_bus, Some(BusId(5)));
        close(a.net_interchange, 100.0);
        close(a.tolerance, 10.0);
        assert_eq!(a.name.as_deref(), Some("AREA-ONE"));

        // Round trip: write and re-read keeps the interchange and swing bus.
        let net2 = parse_psse(&write_psse(&net).text).unwrap();
        assert_eq!(net2.areas.len(), 1);
        let a2 = &net2.areas[0];
        assert_eq!(a2.number, 1);
        assert_eq!(a2.slack_bus, Some(BusId(5)));
        close(a2.net_interchange, 100.0);
        assert_eq!(a2.name.as_deref(), Some("AREA-ONE"));
    }

    #[test]
    fn reads_and_writes_a_switched_shunt() {
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / x
CASE
COMMENT
1,'B1          ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
3,'B3          ', 230.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
7,'B7          ', 230.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
0 / END OF AREA DATA, BEGIN SWITCHED SHUNT DATA
3, 2, 0, 1, 1.05, 0.95, 7, 100.0, '', 19.0, 2, 25.0, 1, 50.0
0 / END OF SWITCHED SHUNT DATA, BEGIN GNE DEVICE DATA
Q
";
        let net = parse_psse(raw).unwrap();
        assert_eq!(net.shunts.len(), 1);
        let sh = &net.shunts[0];
        assert_eq!(sh.bus, BusId(3));
        close(sh.b, 19.0);
        let c = sh.control.as_ref().expect("switched-shunt control parsed");
        assert_eq!(c.mode, SwitchedShuntMode::Discrete);
        close(c.vhigh, 1.05);
        close(c.vlow, 0.95);
        assert_eq!(c.control_bus, Some(BusId(7)));
        close(c.rmpct, 100.0);
        assert_eq!(c.blocks.len(), 2);
        assert_eq!(c.blocks[0].steps, 2);
        close(c.blocks[0].b, 25.0);
        assert_eq!(c.blocks[1].steps, 1);
        close(c.blocks[1].b, 50.0);

        // Round trip: written to the SWITCHED SHUNT section and re-read intact.
        let text = write_psse(&net).text;
        assert!(text.contains("BEGIN SWITCHED SHUNT DATA"));
        let net2 = parse_psse(&text).unwrap();
        assert_eq!(net2.shunts.len(), 1);
        let c2 = net2.shunts[0]
            .control
            .as_ref()
            .expect("control survives the write");
        assert_eq!(c2.mode, SwitchedShuntMode::Discrete);
        assert_eq!(c2.control_bus, Some(BusId(7)));
        assert_eq!(c2.blocks.len(), 2);
        close(c2.blocks[0].b, 25.0);
        close(net2.shunts[0].b, 19.0);
    }

    #[test]
    fn reads_and_writes_a_two_terminal_dc_line() {
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / x
CASE
COMMENT
1,'B1          ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
4,'B4          ', 230.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
5,'B5          ', 230.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
0 / END OF AREA DATA, BEGIN TWO-TERMINAL DC DATA
'DCLINE1', 1, 2.5, 350.0, 500.0, 0.0, 0.0, 0.0, 'I', 0.0, 20, 1.0
4, 1, 15.0, 5.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.5, 0.51, 0.00625, 0, 0, 0, '1', 0.0
5, 1, 15.0, 5.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.5, 0.51, 0.00625, 0, 0, 0, '1', 0.0
0 / END OF TWO-TERMINAL DC DATA, BEGIN VSC DC LINE DATA
Q
";
        let net = parse_psse(raw).unwrap();
        assert_eq!(net.hvdc.len(), 1, "the two-terminal DC line was read");
        let dc = &net.hvdc[0];
        assert_eq!(dc.from, BusId(4), "rectifier bus is the from end");
        assert_eq!(dc.to, BusId(5), "inverter bus is the to end");
        assert!(dc.in_service);
        close(dc.pf, 350.0);
        close(dc.pt, 350.0);

        // Round trip: write and re-read keeps the buses and the power setpoint.
        let net2 = parse_psse(&write_psse(&net).text).unwrap();
        assert_eq!(net2.hvdc.len(), 1, "the DC line survives the write");
        let dc2 = &net2.hvdc[0];
        assert_eq!(dc2.from, BusId(4));
        assert_eq!(dc2.to, BusId(5));
        assert!(dc2.in_service);
        close(dc2.pf, 350.0);
    }

    #[test]
    fn reads_and_writes_a_regulating_transformer_control() {
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / x
CASE
COMMENT
1,'B1          ', 230.0,3,1,1,1,1.00000,0.0,1.1,0.9,1.1,0.9
2,'B2          ', 138.0,1,1,1,1,1.00000,0.0,1.1,0.9,1.1,0.9
3,'B3          ', 13.8,1,1,1,1,1.00000,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
1, 2, 0, '1', 1, 1, 1, 0, 0, 2, 'REG         ', 1, 1, 1, 0, 1, 0, 1, 0, 1, '            '
0.01, 0.10, 100.0
1.025, 0, 2.5, 100.0, 90.0, 80.0, 1, 3, 1.08, 0.92, 1.05, 0.98, 17, 0, 0, 0, 0
1.0, 0
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
Q
";
        let net = parse_psse(raw).unwrap();
        assert_eq!(net.branches.len(), 1);
        let c = net.branches[0].control.as_ref().expect("control parsed");
        assert_eq!(c.mode, TransformerControlMode::Voltage);
        assert_eq!(c.controlled_bus, Some(BusId(3)));
        close(c.tap_max, 1.08);
        close(c.tap_min, 0.92);
        close(c.band_min, 0.98);
        assert_eq!(c.ntp, 17);
        close(c.mva_base, 100.0);

        // Round trip: write and re-read keeps the control block and the tap/shift.
        let net2 = parse_psse(&write_psse(&net).text).unwrap();
        let c2 = net2.branches[0].control.as_ref().expect("control survives");
        assert_eq!(c2.mode, TransformerControlMode::Voltage);
        assert_eq!(c2.controlled_bus, Some(BusId(3)));
        close(c2.tap_max, 1.08);
        assert_eq!(c2.ntp, 17);
        close(net2.branches[0].tap, 1.025);
        close(net2.branches[0].shift, 2.5);
    }

    #[test]
    fn reads_and_writes_a_three_winding_transformer() {
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / x
CASE
COMMENT
1,'B1          ', 230.0,3,1,1,1,1.00000,0.0,1.1,0.9,1.1,0.9
2,'B2          ', 138.0,1,1,1,1,1.00000,0.0,1.1,0.9,1.1,0.9
3,'B3          ', 13.8,1,1,1,1,1.00000,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
1, 2, 3, '1', 1, 1, 1, 0.0, 0.0, 2, 'T3W         ', 1, 1, 1, 0, 1, 0, 1, 0, 1, '            '
0.01, 0.10, 100.0, 0.02, 0.20, 100.0, 0.03, 0.30, 100.0, 0.98, -1.5
1.0, 230.0, 0.0, 100.0, 90.0, 80.0, 0, 0, 1.1, 0.9, 1.1, 0.9, 33, 0, 0, 0, 0
1.025, 138.0, 0.0, 110.0, 0, 0, 0, 0, 1.1, 0.9, 1.1, 0.9, 33, 0, 0, 0, 0
0.95, 13.8, 30.0, 50.0, 0, 0, 0, 0, 1.1, 0.9, 1.1, 0.9, 33, 0, 0, 0, 0
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
Q
";
        let net = parse_psse(raw).unwrap();
        assert_eq!(
            net.transformers_3w.len(),
            1,
            "the 3-winding record was read"
        );
        assert!(net.branches.is_empty(), "a 3W is not folded into branches");
        let t = &net.transformers_3w[0];
        assert_eq!(
            [t.windings[0].bus, t.windings[1].bus, t.windings[2].bus],
            [BusId(1), BusId(2), BusId(3)]
        );
        close(t.z[0].r, 0.01);
        close(t.z[2].x, 0.30);
        close(t.windings[0].rate_a, 100.0);
        close(t.windings[1].tap, 1.025);
        close(t.windings[2].shift, 30.0);
        close(t.star_vm, 0.98);
        close(t.star_va, -1.5);

        // Round trip: write and re-read keeps the windings and the star voltage.
        let net2 = parse_psse(&write_psse(&net).text).unwrap();
        assert_eq!(net2.transformers_3w.len(), 1);
        assert!(net2.branches.is_empty());
        let t2 = &net2.transformers_3w[0];
        close(t2.z[1].x, 0.20);
        close(t2.windings[2].tap, 0.95);
        close(t2.star_va, -1.5);
        assert_eq!(t2.name.as_deref(), Some("T3W"));
    }

    #[test]
    fn writes_v34_v35_layouts_that_round_trip() {
        let raw = r"0, 100.00, 33, 0, 0, 60.00 / x
CASE
COMMENT
1,'B1          ', 230.0,3,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
2,'B2          ', 230.0,1,1,1,1,1.0,0.0,1.1,0.9,1.1,0.9
0 / END OF BUS DATA, BEGIN LOAD DATA
2,'1',1,1,1,10.0,5.0,0,0,0,0,1,1,0
0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA
0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA
0 / END OF GENERATOR DATA, BEGIN BRANCH DATA
1,2,'1 ',0.01,0.05,0.001,111.0,90.0,80.0,0,0,0,0,1,1,0,1,1
0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA
0 / END OF TRANSFORMER DATA, BEGIN AREA DATA
Q
";
        let net = parse_psse(raw).unwrap();

        for rev in [34u32, 35] {
            let text = write_psse_rev(&net, rev).text;
            // v34+ wraps the globals in a system-wide section with its end marker.
            assert!(
                text.contains("END OF SYSTEM-WIDE DATA, BEGIN BUS DATA"),
                "rev {rev} missing the system-wide marker"
            );
            let header = text.lines().next().unwrap();
            assert!(header.contains(&format!(", {rev}, ")), "header {header:?}");
            // The branch uses the named 12-rating layout (>= 24 comma fields).
            let branch = text.lines().find(|l| l.starts_with("1, 2, '1'")).unwrap();
            assert!(
                branch.split(',').count() >= 24,
                "rev {rev} branch is not the named layout: {branch:?}"
            );

            let back = parse_psse(&text).unwrap();
            assert_eq!(back.buses.len(), 2);
            assert_eq!(back.loads.len(), 1);
            assert_eq!(back.branches.len(), 1);
            close(back.branches[0].rate_a, 111.0);
            close(back.loads[0].p, 10.0);
            assert!(back.branches[0].in_service);
        }

        // The v35 load record carries the trailing LOADTYPE field.
        assert!(
            write_psse_rev(&net, 35).text.contains(", ''"),
            "v35 load should carry a LOADTYPE field"
        );
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
