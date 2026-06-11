//! Read PowerWorld `.pwb` binary case files (read only).
//!
//! The format is undocumented; everything here was established by
//! differential analysis of `.pwb`/`.aux` sibling exports of the ACTIVSg
//! synthetic grids and is recorded with its evidence in
//! `docs/powerworld.md`. The reader decodes the power flow core tables
//! (buses, loads, generators, shunts, branches) and stops there; the rest of
//! the file (substations, areas, contingencies, options) is inventoried in
//! the docs and left undecoded.
//!
//! Robustness contract: every record is validated as it is parsed (bus
//! references must exist, floats must be finite and in range, record flags
//! must be values this reader has seen and verified). A file that does not
//! match the validated layout fails loudly; nothing is guessed silently.
//!
//! Supported vintages, all behind header format constant 425: the Simulator
//! 19 era writer (bus record flag family `0x06`, the June 2016 ACTIVSg2000
//! export, validated field by field against its same day aux sibling) and
//! the Simulator 20 era writer (flag family `0x26`: the 2018 ACTIVSg200
//! export validated against its aux, the 2017 v19 ACTIVSg2000 export
//! validated against the published case). Record layouts are self
//! describing through their flag words, a Delphi field presence bitmask:
//! one record model decodes every observed flag word (see [`BusHead::unk`]
//! and [`read_branch_head`]), and structural anchors (the rating block tag)
//! turn any unobserved variant into a loud error instead of a misread.
//! Newer writers (header constants 483 through 551) are classified and
//! rejected with the constant named. Evidence in `docs/powerworld.md`.
//!
//! Known limits, documented rather than guessed:
//!
//! - Status bytes: every device in every available case is in service, so
//!   no out of service encoding is validated anywhere and every device
//!   reads as in service. The load record's post ID byte, once treated as
//!   a status, is 0x00 in the 425 era files and 0x01 in the 483 era one
//!   with every load Closed in both, so it is no status byte; the
//!   generator, shunt, and branch status bytes are unlocated.
//! - Transformer phase shift: every available case has zero phase, so the
//!   field's offset is unknown; transformers read with `shift = 0`.
//! - The slack designation is not stored in the bus record; buses read as
//!   PQ/PV (from the generators) and no bus is marked `Ref`.
//! - The system MVA base is not decoded; per unit values are converted with
//!   the 100 MVA default.
//! - The shunt record's nominal MW slot is unlocated: every available case
//!   stores zero shunt MW, and the slot once assumed to hold it carries 0.99
//!   in the 2016 export (a regulation target, not a power). Shunts read with
//!   `g = 0` and only the nominal MVAr is decoded.
//! - Branch ratings beyond the inline slots (two or three, by flag bit 1)
//!   are zero in every available case; the trailing rating block is
//!   validated as zero filled f32s and read as zero ratings.

use std::collections::HashSet;

use super::map::{BRANCH_DEVICE_TYPE, LINE_CIRCUIT, derive_bus_kinds};
use crate::network::{
    Branch, Bus, BusId, BusType, Extras, Generator, Load, Network, Shunt, SourceFormat,
};
use crate::{Error, Result};

const FMT: &str = "PowerWorld .pwb";

/// The system MVA base used to convert the file's per unit f32 storage into
/// physical units. The base itself is not decoded (see the module docs);
/// every available sibling case uses PowerWorld's 100 MVA default.
const MVA_BASE: f64 = 100.0;

/// How far ahead a bounded scan may look for the next record or table. Large
/// enough for every observed record tail, small enough that a derailed parse
/// fails fast instead of wandering.
const RESYNC_WINDOW: usize = 1024;

/// Parse `.pwb` bytes into a [`Network`]. `name_hint` (the file stem) names
/// the network; the binary carries no case name in the decoded region.
///
/// # Errors
/// [`Error::FormatRead`] when the header is not the known magic, a record
/// does not match the validated layouts, or a table cannot be located.
pub fn parse_pwb(bytes: &[u8], name_hint: Option<&str>) -> Result<Network> {
    let mut cur = Cur { b: bytes, pos: 0 };
    cur.expect_header()?;
    reject_unsupported_vintage(bytes)?;

    // A count word can be forged by record interiors and the case
    // description, so table location is a depth first search: a candidate at
    // any stage is kept only if every later table parses behind it, and the
    // first full chain wins. Wrong candidates die fast on their bounded
    // windows; a file with no valid chain fails loudly.
    for (buses, bus_end) in bus_table_candidates(bytes) {
        let bus_ids: HashSet<usize> = buses.iter().map(|b| b.id.0).collect();
        if bus_ids.len() != buses.len() {
            continue; // duplicate ids: not a real bus table
        }
        for (loads, l_end) in device_table_candidates(bytes, bus_end, &bus_ids, read_load) {
            for (generators, g_end) in device_table_candidates(bytes, l_end, &bus_ids, read_gen) {
                for (shunts, s_end) in device_table_candidates(bytes, g_end, &bus_ids, read_shunt) {
                    let Ok(branches) = walk_branches(bytes, s_end, &bus_ids) else {
                        continue;
                    };
                    let mut buses = buses.clone();
                    derive_bus_kinds(&mut buses, &generators);
                    let net = Network {
                        name: name_hint.unwrap_or("case").to_string(),
                        base_mva: MVA_BASE,
                        buses,
                        loads,
                        shunts,
                        branches,
                        generators,
                        storage: Vec::new(),
                        hvdc: Vec::new(),
                        source_format: SourceFormat::PowerWorldBinary,
                        source: None,
                    };
                    net.check_references(FMT)?;
                    return Ok(net);
                }
            }
        }
    }
    Err(Error::FormatRead {
        format: FMT,
        message: "no table chain matches the validated .pwb layouts \
                  (buses, loads, generators, shunts, branches in sequence)"
            .into(),
    })
}

// ---- Cursor -----------------------------------------------------------------

struct Cur<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Cur<'a> {
    fn err(&self, message: impl Into<String>) -> Error {
        Error::FormatRead {
            format: FMT,
            message: format!("offset {:#x}: {}", self.pos, message.into()),
        }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.pos + n > self.b.len() {
            return Err(self.err(format!("truncated: needed {n} bytes")));
        }
        let s = &self.b[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    fn f32(&mut self) -> Result<f64> {
        Ok(f64::from(f32::from_le_bytes(
            self.take(4)?.try_into().unwrap(),
        )))
    }
    fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    /// A u32 length prefixed string of printable ASCII, at most `max` bytes.
    fn string(&mut self, max: usize) -> Result<String> {
        let n = self.u32()? as usize;
        if n > max {
            return Err(self.err(format!("string length {n} exceeds {max}")));
        }
        let s = self.take(n)?;
        if !printable(s) {
            return Err(self.err("string has non printable bytes"));
        }
        Ok(String::from_utf8_lossy(s).into_owned())
    }

    /// A Pascal ShortString (one length byte), printable, at most `max` bytes.
    fn short_string(&mut self, max: usize) -> Result<String> {
        let n = self.u8()? as usize;
        if n > max {
            return Err(self.err(format!("device ID length {n} exceeds {max}")));
        }
        let s = self.take(n)?;
        if !printable(s) {
            return Err(self.err("device ID has non printable bytes"));
        }
        Ok(String::from_utf8_lossy(s).into_owned())
    }

    fn expect_header(&mut self) -> Result<()> {
        let bad = |c: &Self| {
            c.err(
                "not a recognized PowerWorld binary case (header magic mismatch); \
                 only the validated .pwb layouts are read",
            )
        };
        if self.b.len() < 0x40 {
            return Err(bad(self));
        }
        let (a, v, c) = (self.u64()?, self.u64()?, self.u64()?);
        if a != 15000 {
            return Err(bad(self));
        }
        // Every known PowerWorld binary starts with 15000; the next words
        // identify the writer. 425/20 is the decoded era; 483 through 551
        // appear in 2020-2022 era exports, and older Simulators use other
        // constants or a different header shape entirely.
        if (v, c) != (425, 20) {
            return Err(unsupported_vintage(format!(
                "header format words ({v}, {c}); (425, 20) era files are the \
                 decoded ones"
            )));
        }
        Ok(())
    }
}

fn printable(s: &[u8]) -> bool {
    s.iter().all(|&c| (0x20..0x7f).contains(&c))
}

/// Reject files whose leading 64 KiB carries no run of validated bus record
/// heads, before the table search grinds to a generic "no chain" error. Both
/// decoded record families share the head layout, so a census over
/// [`known_bus_flags`] words finding fewer than two heads means an
/// unrecognized layout, not a sparse case.
fn reject_unsupported_vintage(b: &[u8]) -> Result<()> {
    let scan = b.len().min(0x10000).saturating_sub(8);
    let mut heads = 0usize;
    let mut at = 0x20;
    while at < scan {
        let Ok((_, after)) = read_bus_head(b, at) else {
            at += 1;
            continue;
        };
        heads += 1;
        if heads >= 2 {
            return Ok(());
        }
        at = after;
    }
    Err(unsupported_vintage(
        "no recognized bus record layout in the leading 64 KiB",
    ))
}

/// The single rejection path for recognized-but-undecoded writer vintages;
/// every message names the detected evidence and points at the docs.
fn unsupported_vintage(detail: impl std::fmt::Display) -> Error {
    Error::FormatRead {
        format: FMT,
        message: format!(
            "unsupported PowerWorld .pwb vintage: {detail}; only the validated 425 era \
             record layouts are decoded (see docs/powerworld.md)"
        ),
    }
}

// ---- Bus table --------------------------------------------------------------

struct BusHead {
    bus: Bus,
    /// The flags u32 between name and nominal kV: a Delphi field presence
    /// bitmask, not a per file constant. Bit 5 set marks the Simulator 20
    /// era record family (clear on the Simulator 19 era 0x06/0x07 family,
    /// whose tails are shorter), bit 4 set marks a count prefixed list in
    /// the record tail (2016/2017 era exports), bit 0 clear means one extra
    /// u16 sits before the nominal kV (observed on generator buses). The
    /// 2019+ era writers add bit 6 (constant within a file) and the per
    /// record bit 8; both put their fields in the undecoded tail.
    unk: u32,
}

/// Whether a bus record flag word is one this reader decodes: base bits
/// `0x06` plus any combination of bits 0, 4, 5, 6, and 8 (see
/// [`BusHead::unk`]); the census table in docs/powerworld.md records the
/// observed words. Bits 6 and 8 are the 2019+ era writers' additions; both
/// leave the head layout untouched (their fields live in the tails the
/// resync skips), proven by full value parity on the ACTIVSg500 and
/// published ACTIVSg2000 exports.
fn known_bus_flags(unk: u32) -> bool {
    unk & !0x171 == 0x06
}

/// The record family bits of a bus flag word: everything but the per record
/// presence bits 0, 4, and 8. One file's bus table never mixes families.
fn bus_family(unk: u32) -> u32 {
    unk & !0x111
}

/// Bus table candidates: each `(count, glue)` position after the header whose
/// record walk succeeds, in scan order. The caller validates each candidate
/// by parsing the tables that must follow it.
fn bus_table_candidates(b: &[u8]) -> impl Iterator<Item = (Vec<Bus>, usize)> + '_ {
    let limit = b.len().saturating_sub(4).min(0x10000);
    (0x20..limit).flat_map(move |at| {
        let count = u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) as usize;
        // Table glue between the count and the first record varies by a few
        // bytes per table and vintage; scan a small window for the record.
        let glues = (count != 0 && count <= 2_000_000).then_some(0..=48);
        glues.into_iter().flatten().filter_map(move |glue| {
            let first = at + 4 + glue;
            let (head0, _) = read_bus_head(b, first).ok()?;
            walk_buses(b, first, count, head0.unk).ok()
        })
    })
}

/// Parse one bus record head at `at`; everything through the voltage angle
/// (the head layout both record families share). Returns the parsed bus and
/// leaves undecoded tail bytes, including the bit 4 list, to the resync.
fn read_bus_head(b: &[u8], at: usize) -> Result<(BusHead, usize)> {
    let mut c = Cur { b, pos: at };
    let num = c.u32()? as usize;
    if num == 0 || num > 99_999_999 {
        return Err(c.err("implausible bus number"));
    }
    let name = c.string(64)?;
    if name.is_empty() {
        return Err(c.err("empty bus name"));
    }
    let unk = c.u32()?;
    if !known_bus_flags(unk) {
        return Err(c.err(format!(
            "bus record flags {unk:#x} not in the validated set"
        )));
    }
    if unk & 1 == 0 {
        let _extra = c.u16()?;
    }
    let kv = c.f32()?;
    if !kv.is_finite() || !(0.0..=10_000.0).contains(&kv) {
        return Err(c.err("implausible nominal kV"));
    }
    let area = c.u32()? as usize;
    let zone = c.u32()? as usize;
    let ba = c.u32()?;
    if area > 100_000_000 || zone > 100_000_000 || ba > 100_000_000 {
        return Err(c.err("implausible area/zone/BA number"));
    }
    let _label = c.string(64)?;
    let vm = c.f64()?;
    let va_rad = c.f64()?;
    if !vm.is_finite() || !(0.0..=10.0).contains(&vm) || !va_rad.is_finite() || va_rad.abs() > 100.0
    {
        return Err(c.err("implausible voltage solution"));
    }
    let bus = Bus {
        id: BusId(num),
        kind: BusType::Pq,
        vm,
        va: va_rad.to_degrees(),
        base_kv: kv,
        vmax: 1.1,
        vmin: 0.9,
        area,
        zone,
        name: Some(name),
        extras: Extras::new(),
    };
    Ok((BusHead { bus, unk }, c.pos))
}

fn walk_buses(b: &[u8], first: usize, count: usize, unk: u32) -> Result<(Vec<Bus>, usize)> {
    // Capacity hint bounded by what the buffer could hold (no record is
    // smaller than 16 bytes), so a forged count cannot pre-allocate big.
    let mut buses = Vec::with_capacity(count.min(b.len().saturating_sub(first) / 16));
    let mut at = first;
    for i in 0..count {
        let (head, after) = read_bus_head(b, at)?;
        if bus_family(head.unk) != bus_family(unk) {
            return Err(Error::FormatRead {
                format: FMT,
                message: format!("bus record {i}: record family changed mid table"),
            });
        }
        buses.push(head.bus);
        if i + 1 == count {
            return Ok((buses, after));
        }
        // The record tail (undecoded; longer when flag bit 4 inserts a count
        // prefixed list) separates this record from the next; find the next
        // head by bounded scan.
        at = resync(after, after + RESYNC_WINDOW, |p| {
            read_bus_head(b, p)
                .ok()
                .filter(|(h, _)| bus_family(h.unk) == bus_family(unk))
                .map(|_| ())
        })
        .ok_or_else(|| Error::FormatRead {
            format: FMT,
            message: format!(
                "bus record {}: next record not found after {after:#x}",
                i + 1
            ),
        })?;
    }
    // count == 0 only (the candidate scans filter it out): an empty table
    // ends where it began.
    Ok((buses, first))
}

/// First position in `[from, to)` where `probe` accepts.
fn resync(from: usize, to: usize, probe: impl Fn(usize) -> Option<()>) -> Option<usize> {
    (from..to).find(|&p| probe(p).is_some())
}

// ---- Bus + ShortString ID tables (loads, generators, shunts) ----------------

/// One record of a device table keyed by bus + ShortString ID. `read` parses
/// the record head at the cursor (bus and ID already consumed) and returns
/// the element.
type ReadDevice<T> = fn(&mut Cur, BusId, String) -> Result<T>;

fn read_device_head<T>(
    b: &[u8],
    at: usize,
    bus_ids: &HashSet<usize>,
    read: ReadDevice<T>,
) -> Result<(T, usize)> {
    let mut c = Cur { b, pos: at };
    let bus = c.u32()? as usize;
    if !bus_ids.contains(&bus) {
        return Err(c.err("record references an unknown bus"));
    }
    let id = c.short_string(8)?;
    if id.is_empty() {
        return Err(c.err("empty device ID"));
    }
    let v = read(&mut c, BusId(bus), id)?;
    Ok((v, c.pos))
}

/// Candidates for a count prefixed device table after `from`: every
/// `(count, glue)` whose full record walk succeeds, in scan order. The caller
/// keeps a candidate only if the tables that must follow it parse too.
fn device_table_candidates<'a, T: 'a>(
    b: &'a [u8],
    from: usize,
    bus_ids: &'a HashSet<usize>,
    read: ReadDevice<T>,
) -> impl Iterator<Item = (Vec<T>, usize)> + 'a {
    let limit = (from + RESYNC_WINDOW).min(b.len().saturating_sub(4));
    (from..limit).flat_map(move |at| {
        let count = u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) as usize;
        let glues = (count != 0 && count <= 10_000_000).then_some(0..=48);
        glues.into_iter().flatten().filter_map(move |glue| {
            let first = at + 4 + glue;
            if first >= b.len() {
                return None;
            }
            read_device_head(b, first, bus_ids, read).ok()?;
            walk_devices(b, first, count, bus_ids, read).ok()
        })
    })
}

fn walk_devices<T>(
    b: &[u8],
    first: usize,
    count: usize,
    bus_ids: &HashSet<usize>,
    read: ReadDevice<T>,
) -> Result<(Vec<T>, usize)> {
    let mut out = Vec::with_capacity(count.min(b.len().saturating_sub(first) / 16));
    let mut at = first;
    for i in 0..count {
        let (v, after) = read_device_head(b, at, bus_ids, read)?;
        out.push(v);
        if i + 1 == count {
            return Ok((out, after));
        }
        at = resync(after, after + RESYNC_WINDOW, |p| {
            read_device_head(b, p, bus_ids, read).ok().map(|_| ())
        })
        .ok_or_else(|| Error::FormatRead {
            format: FMT,
            message: format!(
                "device record {}: next record not found after {after:#x}",
                i + 1
            ),
        })?;
    }
    // count == 0 only (the candidate scans filter it out): an empty table
    // ends where it began.
    Ok((out, first))
}

/// Load record: one undecoded byte, then constant power P and Q in per unit
/// (f32). The byte is 0x00 in every 425 era record and 0x01 in every 483
/// era one while both auxes say every load is Closed, so it is not a status
/// byte; loads read as in service (see the module docs).
fn read_load(c: &mut Cur, bus: BusId, id: String) -> Result<Load> {
    let _flag = c.u8()?;
    let p = c.f32()? * MVA_BASE;
    let q = c.f32()? * MVA_BASE;
    if !p.is_finite() || !q.is_finite() || p.abs() > 1.0e6 || q.abs() > 1.0e6 {
        return Err(c.err("implausible load power"));
    }
    let mut extras = Extras::new();
    extras.insert("LoadID".into(), serde_json::Value::String(id));
    Ok(Load {
        bus,
        p,
        q,
        in_service: true,
        extras,
    })
}

/// Generator record: the ID is a fixed capacity ShortString[2] (so the
/// payload sits at constant offsets from the record start), undecoded flag
/// bytes, then eight consecutive f32s: MW setpoint, MVAr setpoint, MVAr
/// max, MVAr min (per unit), voltage setpoint (p.u.), MVA base, MW max, MW
/// min (per unit). The f32 block starts at +9 or +10 in the 2016/2017
/// exports (the flag bytes before it vary per record) and +11 in the 2018
/// one; the voltage setpoint and MVA base ranges anchor the choice, and a
/// record that puts implausible values at every offset is a loud error, not
/// a generator.
#[allow(clippy::needless_pass_by_value)] // the ReadDevice fn type fixes the signature
fn read_gen(c: &mut Cur, bus: BusId, id: String) -> Result<Generator> {
    let record_start = c.pos - (4 + 1) - id.len(); // u32 bus + the ID length byte
    let mut chosen = None;
    // +12 extends the observed set to two character IDs in a 2018 era
    // export (unobserved, but the pre-rework probe covered it).
    for anchor in [9usize, 10, 11, 12] {
        let mut probe = Cur {
            b: c.b,
            pos: record_start + anchor,
        };
        let Ok(vals) = (0..8).map(|_| probe.f32()).collect::<Result<Vec<_>>>() else {
            continue;
        };
        let plausible = vals.iter().all(|x| x.is_finite() && x.abs() < 1.0e6)
            && (0.5..=1.6).contains(&vals[4])
            && (0.1..=1.0e5).contains(&vals[5]);
        if plausible {
            chosen = Some((vals, probe.pos));
            break;
        }
    }
    let Some((v, end)) = chosen else {
        return Err(c.err("generator record does not match the validated layouts"));
    };
    c.pos = end;
    Ok(Generator {
        bus,
        pg: v[0] * MVA_BASE,
        qg: v[1] * MVA_BASE,
        qmax: v[2] * MVA_BASE,
        qmin: v[3] * MVA_BASE,
        vg: v[4],
        mbase: v[5],
        pmax: v[6] * MVA_BASE,
        pmin: v[7] * MVA_BASE,
        // The status byte is unlocated within the +7 flag bytes; every
        // available machine is Closed (see the module docs).
        in_service: true,
        cost: None,
        caps: Default::default(),
    })
}

/// Shunt record: nominal MVAr as f32 at +24 from the record start, validated
/// on all 199 shunts across the three sibling cases. The slot at +20 is 0.0
/// in the Simulator 20 era files but 0.99 in the 2016 export, so it is not
/// the nominal MW (see the module docs); shunts read with `g = 0`.
fn read_shunt(c: &mut Cur, bus: BusId, id: String) -> Result<Shunt> {
    let record_start = c.pos - (4 + 1) - id.len(); // u32 bus + the ID length byte
    let mut probe = Cur {
        b: c.b,
        pos: record_start + 24,
    };
    let b_mvar = probe.f32()? * MVA_BASE;
    if !b_mvar.is_finite() || b_mvar.abs() > 1.0e6 {
        return Err(c.err("implausible shunt MVAr"));
    }
    c.pos = probe.pos;
    let mut extras = Extras::new();
    extras.insert("ShuntID".into(), serde_json::Value::String(id));
    Ok(Shunt {
        bus,
        // The nominal MW slot is unlocated (every available case stores
        // zero); see the module docs.
        g: 0.0,
        b: b_mvar,
        in_service: true,
        extras,
    })
}

// ---- Branch table ------------------------------------------------------------

/// Whether a branch record flag word is one this reader decodes: base bits
/// `0xEC` plus any combination of bits 0, 1, and 4, a Delphi field presence
/// bitmask like the bus record's. Bit 0 set omits the circuit ID string and
/// its status byte (the PowerWorld default " 1" applies), bit 1 set means
/// two inline rating slots instead of three (the Simulator 19 era writer
/// inlines three), bit 4 marks a count prefixed list in the record tail.
/// Observed words: 0xEC/0xFC (2016), 0xEE/0xEF (2018 and v19), 0xFE/0xFF
/// (v19); the remaining two combinations are admitted by the same bit
/// logic and guarded by the structural anchors in [`read_branch_head`].
fn known_branch_flags(flags: u16) -> bool {
    flags & !0x13 == 0x00EC
}

fn walk_branches(b: &[u8], from: usize, bus_ids: &HashSet<usize>) -> Result<Vec<Branch>> {
    // The gap between the shunt table end and the branch count word can
    // exceed one resync window; two cover every observed file.
    let limit = (from + RESYNC_WINDOW * 2).min(b.len().saturating_sub(4));
    for at in from..limit {
        let count = u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) as usize;
        if count == 0 || count > 10_000_000 {
            continue;
        }
        // The branch table glue is longer than the device tables'; scan a
        // window after the count for the first record.
        let Some(first) = resync(at + 4, (at + 64).min(b.len()), |p| {
            read_branch_head(b, p, bus_ids).ok().map(|_| ())
        }) else {
            continue;
        };
        if let Ok((branches, after)) = walk_branch_records(b, first, count, bus_ids) {
            // Reject a count that is a prefix of the real table: if another
            // branch record follows within the resync window, the table did
            // not actually stop here (a forged count word inside the glue can
            // parse its first records). The true count lands where no further
            // branch record follows.
            let continues = resync(after, after + RESYNC_WINDOW, |p| {
                read_branch_head(b, p, bus_ids).ok().map(|_| ())
            })
            .is_some();
            if !continues {
                return Ok(branches);
            }
        }
    }
    Err(Error::FormatRead {
        format: FMT,
        message: format!("branch table not found after {from:#x}"),
    })
}

fn walk_branch_records(
    b: &[u8],
    first: usize,
    count: usize,
    bus_ids: &HashSet<usize>,
) -> Result<(Vec<Branch>, usize)> {
    let mut out = Vec::with_capacity(count.min(b.len().saturating_sub(first) / 16));
    let mut at = first;
    for i in 0..count {
        let (br, after) = read_branch_head(b, at, bus_ids)?;
        out.push(br);
        if i + 1 == count {
            return Ok((out, after));
        }
        at = resync(after, after + RESYNC_WINDOW, |p| {
            read_branch_head(b, p, bus_ids).ok().map(|_| ())
        })
        .ok_or_else(|| Error::FormatRead {
            format: FMT,
            message: format!(
                "branch record {}: next record not found after {after:#x}",
                i + 1
            ),
        })?;
    }
    // count == 0 only (walk_branches filters it out): an empty table ends
    // where it began.
    Ok((out, first))
}

/// Branch record, validated field by field against the aux siblings of all
/// three cases (6,491 records). After the impedances: two or three inline
/// per unit rating slots (by flag bit 1), a constant u32 tag, eleven f32
/// slots (zero in every available case), one zero byte, then the kind byte
/// that separates lines from transformers (which carry their tap next). The
/// tag and the zero byte are structural anchors: an unobserved variant
/// shifts them and dies loudly instead of misreading.
#[allow(clippy::many_single_char_names)] // r, x, b are the domain names
fn read_branch_head(b: &[u8], at: usize, bus_ids: &HashSet<usize>) -> Result<(Branch, usize)> {
    let mut c = Cur { b, pos: at };
    let from = c.u32()? as usize;
    let to = c.u32()? as usize;
    if !bus_ids.contains(&from) || !bus_ids.contains(&to) || from == to {
        return Err(c.err("branch references unknown buses"));
    }
    let flags = c.u16()?;
    if !known_branch_flags(flags) {
        return Err(c.err(format!(
            "branch record flags {flags:#06x} not in the validated set; unsupported .pwb variant"
        )));
    }
    let circuit = if flags & 1 == 0 {
        // The circuit ID is a Delphi ShortString[2]: one length byte and a
        // fixed two byte text area (a one character ID leaves the second
        // byte unused). Establishing this took the v19 file's parallel
        // circuit records, the first in the corpus with two character IDs.
        let n = c.u8()? as usize;
        if n == 0 || n > 2 {
            return Err(c.err(format!("circuit ID length {n} (expected 1 or 2)")));
        }
        let text = c.take(2)?;
        if !printable(&text[..n]) {
            return Err(c.err("circuit ID has non printable bytes"));
        }
        String::from_utf8_lossy(&text[..n]).into_owned()
    } else {
        // Omitted circuit: PowerWorld's default, observed as " 1" in the
        // sibling aux.
        " 1".to_string()
    };
    let r = c.f32()?;
    let x = c.f32()?;
    let b_chg = c.f32()?;
    for v in [r, x, b_chg] {
        if !v.is_finite() || v.abs() > 1.0e4 {
            return Err(c.err("implausible branch impedance"));
        }
    }
    let _g = c.f32()?;
    let inline = if flags & 2 == 0 { 3 } else { 2 };
    let mut rates = [0.0f64; 3];
    for slot in rates.iter_mut().take(inline) {
        let v = c.f32()?;
        if !v.is_finite() || !(0.0..=1.0e6).contains(&v) {
            return Err(c.err("implausible branch rating"));
        }
        *slot = v * MVA_BASE;
    }
    let tag = c.u32()?;
    if tag != 12 {
        return Err(c.err(format!(
            "branch rating block tag {tag} (expected 12); unvalidated .pwb branch variant"
        )));
    }
    for _ in 0..11 {
        let v = c.f32()?;
        if !v.is_finite() || v.abs() > 1.0e6 {
            return Err(c.err("implausible branch rating block value"));
        }
    }
    if c.u8()? != 0 {
        return Err(c.err("branch record separator byte not zero"));
    }
    let kind = c.u8()?;
    let (device, tap) = match kind {
        0x01 => ("Line", 0.0),
        0x00 => {
            let tap = c.f32()?;
            if !tap.is_finite() || !(0.2..=5.0).contains(&tap) {
                return Err(c.err("implausible transformer tap"));
            }
            ("Transformer", tap)
        }
        other => {
            return Err(c.err(format!(
                "branch kind marker {other:#04x} not in the validated set"
            )));
        }
    };
    let mut extras = Extras::new();
    extras.insert(LINE_CIRCUIT.into(), serde_json::Value::String(circuit));
    extras.insert(
        BRANCH_DEVICE_TYPE.into(),
        serde_json::Value::String(device.into()),
    );
    let br = Branch {
        from: BusId(from),
        to: BusId(to),
        r,
        x,
        b: b_chg,
        rate_a: rates[0],
        rate_b: rates[1],
        rate_c: rates[2],
        tap,
        // Phase shift is undecoded: every available case has zero phase, so
        // the field's location is unknown (see the module docs).
        shift: 0.0,
        // The branch status byte is unlocated (the byte once assumed to be
        // it was the circuit ID's unused capacity byte); every available
        // record is Closed. See the module docs.
        in_service: true,
        angmin: -360.0,
        angmax: 360.0,
        extras,
    };
    Ok((br, c.pos))
}
