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
//! Supported vintage: the Simulator 20 era writer with plain record tails
//! (bus record flag word `0x26`/`0x27`), validated field by field against the
//! ACTIVSg200 aux sibling. Older writers share the bus record head layout but
//! differ behind it: the Simulator 19 era family (flag `0x06`/`0x07`, the
//! June 2016 ACTIVSg2000 export) uses different record tails, and 2016/2017
//! era exports carry count prefixed lists in some tails (flag bit 4, the v19
//! file PowerWorld hosts). A census of bus record heads classifies the writer
//! family, and anything beyond the decoded layout is rejected with the
//! evidence named rather than decoded partially. Extending coverage is a
//! documented follow-up; see `docs/powerworld.md`.
//!
//! Known limits, documented rather than guessed:
//!
//! - Status bytes: every device in the available sibling cases is Closed
//!   (byte 0). A nonzero byte is read as out of service, which follows the
//!   Delphi boolean convention but has no validated sample.
//! - Transformer phase shift: every available case has zero phase, so the
//!   field's offset is unknown; transformers read with `shift = 0`.
//! - The slack designation is not stored in the bus record; buses read as
//!   PQ/PV (from the generators) and no bus is marked `Ref`.
//! - The system MVA base is not decoded; per unit values are converted with
//!   the 100 MVA default.

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
        if (a, c) != (15000, 20) {
            return Err(bad(self));
        }
        // The middle u64 is the writer format constant: 425 in every decoded
        // file; 483/508/537/550/551 observed in 2021/2022 era exports.
        if v != 425 {
            return Err(unsupported_vintage(format!(
                "header format constant {v} (a newer writer; 425 era files are \
                 the decoded ones)"
            )));
        }
        Ok(())
    }
}

fn printable(s: &[u8]) -> bool {
    s.iter().all(|&c| (0x20..0x7f).contains(&c))
}

/// Classify the writer vintage from a census of validated bus record heads
/// in the leading 64 KiB and reject what the table walk cannot decode, naming
/// the evidence. Every known vintage shares the bus record head layout
/// through the solved voltage, so heads parse even where the tails do not;
/// the flag word census then identifies the writer family (see
/// [`KNOWN_BUS_FLAGS`]). Rejecting here gives the caller the detected vintage
/// instead of letting the table search grind to a generic "no chain" error.
fn reject_unsupported_vintage(b: &[u8]) -> Result<()> {
    let scan = b.len().min(0x10000).saturating_sub(8);
    let (mut sim19, mut sim20, mut list_tails) = (0usize, 0usize, 0usize);
    let mut at = 0x20;
    while at < scan {
        let Ok((head, after)) = read_bus_head(b, at, &KNOWN_BUS_FLAGS) else {
            at += 1;
            continue;
        };
        if head.unk & 0x20 == 0 {
            sim19 += 1;
        } else {
            sim20 += 1;
        }
        if head.unk & 0x10 != 0 {
            list_tails += 1;
        }
        at = after;
    }
    let detail = if sim19 > sim20 {
        "Simulator 19 era bus records (flag words 0x06/0x07)".into()
    } else if list_tails > 0 {
        format!(
            "{list_tails} bus record tails carry count prefixed lists \
             (flag words 0x36/0x37, seen in 2016/2017 era exports)"
        )
    } else if sim20 >= 2 {
        return Ok(());
    } else {
        "no recognized bus record layout in the leading 64 KiB".into()
    };
    Err(unsupported_vintage(detail))
}

/// The single rejection path for recognized-but-undecoded writer vintages;
/// every message names the detected evidence and points at the docs.
fn unsupported_vintage(detail: impl std::fmt::Display) -> Error {
    Error::FormatRead {
        format: FMT,
        message: format!(
            "unsupported PowerWorld .pwb vintage: {detail}; only the Simulator 20 era \
             layout (header format constant 425) with plain record tails is decoded (see docs/powerworld.md)"
        ),
    }
}

// ---- Bus table --------------------------------------------------------------

struct BusHead {
    bus: Bus,
    /// The flags u32 between name and nominal kV: a field presence bitmask,
    /// not a per file constant. Bit 5 set marks the Simulator 20 era record
    /// family (clear on the 2016 era 0x06/0x07 family), bit 4 set marks a
    /// count prefixed list in the record tail, bit 0 clear means one extra
    /// u16 sits before the nominal kV (observed on generator buses).
    unk: u32,
}

/// Bus record flag words this reader decodes: the Simulator 20 era family
/// with plain record tails. [`reject_unsupported_vintage`] turns everything
/// else away early with the detected family named.
const BUS_FLAGS: [u32; 2] = [0x26, 0x27];

/// Every bus record flag word observed across the corpus (the census table
/// in docs/powerworld.md), decoded or not
/// (see [`BusHead::unk`] for the bit meanings). The census in
/// [`reject_unsupported_vintage`] accepts all of them to identify the writer
/// family; the table walk accepts only [`BUS_FLAGS`].
const KNOWN_BUS_FLAGS: [u32; 8] = [0x06, 0x07, 0x16, 0x17, 0x26, 0x27, 0x36, 0x37];

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
            let (head0, _) = read_bus_head(b, first, &BUS_FLAGS).ok()?;
            walk_buses(b, first, count, head0.unk).ok()
        })
    })
}

/// Parse one bus record head at `at`; everything through the voltage angle
/// (the head layout every known vintage shares). `accept` is the flag word
/// set to admit: [`BUS_FLAGS`] when decoding, [`KNOWN_BUS_FLAGS`] when taking
/// the vintage census. Returns the parsed bus and leaves undecoded tail bytes
/// to the resync.
fn read_bus_head(b: &[u8], at: usize, accept: &[u32]) -> Result<(BusHead, usize)> {
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
    if !accept.contains(&unk) {
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
        let (head, after) = read_bus_head(b, at, &BUS_FLAGS)?;
        if head.unk | 1 != unk | 1 {
            return Err(Error::FormatRead {
                format: FMT,
                message: format!("bus record {i}: vintage marker changed mid table"),
            });
        }
        buses.push(head.bus);
        if i + 1 == count {
            return Ok((buses, after));
        }
        // The record tail (constant per file, undecoded) separates this
        // record from the next; find the next head by bounded scan.
        at = resync(after, after + RESYNC_WINDOW, |p| {
            read_bus_head(b, p, &BUS_FLAGS)
                .ok()
                .filter(|(h, _)| h.unk | 1 == unk | 1)
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

/// Load record: status byte, then constant power P and Q in per unit (f32).
fn read_load(c: &mut Cur, bus: BusId, id: String) -> Result<Load> {
    let status = c.u8()?;
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
        in_service: status == 0,
        extras,
    })
}

/// Generator record: a small gap whose width varies by writer vintage (3 to
/// 5 bytes; the first byte is the status), then eight consecutive f32s:
/// MW setpoint, MVAr setpoint, MVAr max, MVAr min (per unit), voltage
/// setpoint (p.u.), MVA base, MW max, MW min (per unit).
fn read_gen(c: &mut Cur, bus: BusId, _id: String) -> Result<Generator> {
    let status = c.u8()?;
    // Calibrate the vintage gap: after skipping 2 to 4 more bytes the f32
    // block must give a plausible voltage setpoint and MVA base.
    let base = c.pos;
    let mut chosen = None;
    for skip in [2usize, 3, 4] {
        let mut probe = Cur {
            b: c.b,
            pos: base + skip,
        };
        let Ok(vals) = (0..8).map(|_| probe.f32()).collect::<Result<Vec<_>>>() else {
            continue;
        };
        let vg = vals[4];
        let mbase = vals[5];
        let plausible = vals.iter().all(|v| v.is_finite() && v.abs() < 1.0e6)
            && (0.5..=1.6).contains(&vg)
            && (0.1..=1.0e5).contains(&mbase);
        if plausible {
            chosen = Some((skip, vals, probe.pos));
            break;
        }
    }
    let Some((_, v, end)) = chosen else {
        return Err(c.err("generator record does not match a validated layout"));
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
        in_service: status == 0,
        cost: None,
        caps: Default::default(),
    })
}

/// Shunt record: nominal MW at +20 and MVAr at +24 from the record start
/// (offsets identical in both vintages; the MW slot is zero in every
/// available case and its position is inferred from adjacency).
fn read_shunt(c: &mut Cur, bus: BusId, id: String) -> Result<Shunt> {
    let record_start = c.pos - (4 + 1) - id.len(); // u32 bus + the ID length byte
    let mut probe = Cur {
        b: c.b,
        pos: record_start + 20,
    };
    let g = probe.f32()? * MVA_BASE;
    let b_mvar = probe.f32()? * MVA_BASE;
    if !g.is_finite() || !b_mvar.is_finite() || g.abs() > 1.0e6 || b_mvar.abs() > 1.0e6 {
        return Err(c.err("implausible shunt values"));
    }
    c.pos = probe.pos;
    let mut extras = Extras::new();
    extras.insert("ShuntID".into(), serde_json::Value::String(id));
    Ok(Shunt {
        bus,
        g,
        b: b_mvar,
        in_service: true,
        extras,
    })
}

// ---- Branch table ------------------------------------------------------------

/// Branch record flag words this reader has validated. Bit 0 set means the
/// circuit ID string (and its status byte) is omitted and the PowerWorld
/// default " 1" applies; bit 1 differs between writer vintages with no
/// structural change.
const BRANCH_FLAGS: std::ops::RangeInclusive<u16> = 0x00EC..=0x00EF;

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

#[allow(clippy::many_single_char_names)] // r, x, b are the domain names
fn read_branch_head(b: &[u8], at: usize, bus_ids: &HashSet<usize>) -> Result<(Branch, usize)> {
    let mut c = Cur { b, pos: at };
    let from = c.u32()? as usize;
    let to = c.u32()? as usize;
    if !bus_ids.contains(&from) || !bus_ids.contains(&to) || from == to {
        return Err(c.err("branch references unknown buses"));
    }
    let flags = c.u16()?;
    if !BRANCH_FLAGS.contains(&flags) {
        return Err(c.err(format!(
            "branch record flags {flags:#06x} not in the validated set; unsupported .pwb variant"
        )));
    }
    let (circuit, status) = if flags & 1 == 0 {
        let ckt = c.short_string(8)?;
        if ckt.is_empty() {
            return Err(c.err("empty circuit ID"));
        }
        (ckt, c.u8()?)
    } else {
        // Omitted circuit: PowerWorld's default, observed as " 1" in the
        // sibling aux.
        (" 1".to_string(), 0)
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
    let mut rates = [0.0f64; 14];
    for slot in &mut rates {
        let v = c.f32()?;
        if !v.is_finite() || !(0.0..=1.0e6).contains(&v) {
            return Err(c.err("implausible branch rating"));
        }
        *slot = v * MVA_BASE;
    }
    let kind = c.u8()?;
    let (device, tap) = match kind {
        0x01 => ("Line", 0.0),
        0x00 => {
            let _pad = c.u8()?;
            let tap = c.f32()?;
            if !tap.is_finite() || !(0.0..=10.0).contains(&tap) {
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
        in_service: status == 0,
        angmin: -360.0,
        angmax: 360.0,
        extras,
    };
    Ok((br, c.pos))
}
