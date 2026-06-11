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
//! Supported vintages, behind header format constants 425 and 508: the
//! Simulator 19 era writer (bus record flag family `0x06`, the June 2016
//! ACTIVSg2000 export, validated field by field against its same day aux
//! sibling), the Simulator 20 era writer (flag family `0x26`: the 2018
//! ACTIVSg200 export validated against its aux, the 2017 v19 ACTIVSg2000
//! export validated against the published case), and the 2019+ era writers
//! (flag bits 6 and 8 added over family `0x26`, header 425 or 508: the
//! current era ACTIVSg2000, ACTIVSg500, and 2022 Hawaii40 exports, each
//! validated against its aux). Record layouts are self
//! describing through their flag words, a Delphi field presence bitmask:
//! one record model decodes every observed flag word (see [`BusHead::unk`]
//! and [`read_branch_head`]), and structural anchors (the rating block tag)
//! turn any unobserved variant into a loud error instead of a misread.
//! Newer writers (header constants 483 through 551) are classified and
//! rejected with the constant named. Evidence in `docs/powerworld.md`.
//!
//! The table search prices the format's structure (no field dictionary, so
//! every table is located by validating record walks behind count word
//! candidates), and the probe layer is built so that search allocates only
//! for records it accepts: probe rejections carry `&'static str` reasons
//! instead of formatted strings ([`Probe`]), bus membership is a bitmap
//! over the id range instead of a hash set ([`BusIdSet`]), and record runs
//! are cached by first record offset ([`Run`]) so count word candidates
//! that point at the same records share one walk. Issue #99 records the
//! measurements.
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

use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::hash_map::Entry;

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

/// The probe layer's error type. Probe rejections are pure control flow (the
/// table search discards them wholesale and the loud user visible errors are
/// built at the parse boundary), so they carry a static description and never
/// allocate; the texts document why each check exists.
type Probe<T> = std::result::Result<T, &'static str>;

/// Parse `.pwb` bytes into a [`Network`]. `name_hint` (the file stem) names
/// the network; the binary carries no case name in the decoded region.
///
/// # Errors
/// [`Error::FormatRead`] when the header is not the known magic, a record
/// does not match the validated layouts, or a table cannot be located.
pub fn parse_pwb(bytes: &[u8], name_hint: Option<&str>) -> Result<Network> {
    expect_header(bytes)?;
    reject_unsupported_vintage(bytes)?;

    // A count word can be forged by record interiors and the case
    // description, so table location is a depth first search: a candidate at
    // any stage is kept only if every later table parses behind it, and the
    // first full chain wins. Wrong candidates die fast on their bounded
    // windows; a file with no valid chain fails loudly. The run caches make
    // the backtracking affordable: candidates pointing at the same first
    // record share one walk however many count words and search retries
    // reach it.
    let bus_runs = RefCell::new(HashMap::new());
    for (buses, bus_end) in bus_table_candidates(bytes, &bus_runs) {
        let Some(bus_ids) = BusIdSet::new(&buses) else {
            continue; // duplicate ids: not a real bus table
        };
        // The device and branch runs validate bus references, so their
        // caches are scoped to one bus table candidate.
        let load_runs = RefCell::new(HashMap::new());
        let gen_runs = RefCell::new(HashMap::new());
        let shunt_runs = RefCell::new(HashMap::new());
        let branch_runs = RefCell::new(HashMap::new());
        for (loads, l_end) in
            device_table_candidates(bytes, bus_end, &bus_ids, read_load, &load_runs)
        {
            for (generators, g_end) in
                device_table_candidates(bytes, l_end, &bus_ids, read_gen, &gen_runs)
            {
                for (shunts, s_end) in
                    device_table_candidates(bytes, g_end, &bus_ids, read_shunt, &shunt_runs)
                {
                    let Some(branches) = find_branch_table(bytes, s_end, &bus_ids, &branch_runs)
                    else {
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
    fn take(&mut self, n: usize) -> Probe<&'a [u8]> {
        if self.pos + n > self.b.len() {
            return Err("truncated record");
        }
        let s = &self.b[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    fn u8(&mut self) -> Probe<u8> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Probe<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Probe<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn f32(&mut self) -> Probe<f64> {
        Ok(f64::from(f32::from_le_bytes(
            self.take(4)?.try_into().unwrap(),
        )))
    }
    fn f64(&mut self) -> Probe<f64> {
        Ok(f64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    /// A u32 length prefixed string of printable ASCII, at most `max` bytes.
    /// Returns the raw slice; accepted records convert it once, so the
    /// rejected probe offsets (the overwhelming majority) never allocate.
    fn string(&mut self, max: usize) -> Probe<&'a [u8]> {
        let n = self.u32()? as usize;
        if n > max {
            return Err("string length exceeds the field maximum");
        }
        let s = self.take(n)?;
        if !printable(s) {
            return Err("string has non printable bytes");
        }
        Ok(s)
    }

    /// A Pascal ShortString (one length byte), printable, at most `max` bytes.
    fn short_string(&mut self, max: usize) -> Probe<&'a [u8]> {
        let n = self.u8()? as usize;
        if n > max {
            return Err("device ID length exceeds the field maximum");
        }
        let s = self.take(n)?;
        if !printable(s) {
            return Err("device ID has non printable bytes");
        }
        Ok(s)
    }
}

fn printable(s: &[u8]) -> bool {
    s.iter().all(|&c| (0x20..0x7f).contains(&c))
}

fn expect_header(b: &[u8]) -> Result<()> {
    let bad = || Error::FormatRead {
        format: FMT,
        message: "not a recognized PowerWorld binary case (header magic mismatch); \
                  only the validated .pwb layouts are read"
            .into(),
    };
    if b.len() < 0x40 {
        return Err(bad());
    }
    let word = |i: usize| u64::from_le_bytes(b[i * 8..i * 8 + 8].try_into().unwrap());
    let (a, v, c) = (word(0), word(1), word(2));
    if a != 15000 {
        return Err(bad());
    }
    // Every known PowerWorld binary starts with 15000; the next words
    // identify the writer. The decoded eras are 425 (2016 through 2019
    // era record families) and 508 (validated by parity on the 2022
    // Hawaii40 export, whose records use the same bit 6/8 family);
    // 483/537/550/551 exports carry structures the record models do
    // not cover yet, and older Simulators use other constants or a
    // different header shape entirely.
    if c != 20 || !(v == 425 || v == 508) {
        return Err(unsupported_vintage(format!(
            "header format words ({v}, {c}); the decoded eras are \
             (425, 20) and (508, 20)"
        )));
    }
    Ok(())
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

// ---- Search machinery --------------------------------------------------------

/// Bus id membership for the record probes, the hottest check in the table
/// search (every probed byte offset starts with one or two lookups). A
/// bitmap over the id range replaces hashing; [`read_bus_head`] caps ids at
/// 99,999,999 so the map is bounded, and the corpus tops out around 8200.
struct BusIdSet {
    words: Vec<u64>,
}

impl BusIdSet {
    /// `None` when an id repeats: a table with duplicate bus numbers is a
    /// forged candidate, not a real bus table.
    fn new(buses: &[Bus]) -> Option<Self> {
        let max = buses.iter().map(|b| b.id.0).max().unwrap_or(0);
        let mut words = vec![0u64; max / 64 + 1];
        for bus in buses {
            let (w, bit) = (bus.id.0 / 64, 1u64 << (bus.id.0 % 64));
            if words[w] & bit != 0 {
                return None;
            }
            words[w] |= bit;
        }
        Some(Self { words })
    }

    #[inline]
    fn contains(&self, id: usize) -> bool {
        self.words
            .get(id / 64)
            .is_some_and(|w| w & (1 << (id % 64)) != 0)
    }
}

/// The record run from one first record offset: the walk from a given offset
/// is unique, so every count word candidate pointing at the same first
/// record shares it. A count that is a prefix of a longer run reuses the
/// boundaries already walked; a count past the point where extension failed
/// is rejected without rescanning.
struct Run<T> {
    items: Vec<T>,
    /// End offset just past `items[i]`.
    ends: Vec<usize>,
    /// Extension past `items.len()` already failed; never retried.
    dead: bool,
}

impl<T: Clone> Run<T> {
    fn start(item: T, end: usize) -> Self {
        Run {
            items: vec![item],
            ends: vec![end],
            dead: false,
        }
    }

    /// Extend to `count` records if the bytes allow, finding each next
    /// record with `next(after, prev)` (the record tails are undecoded and
    /// vary, so each step is a bounded scan). Returns the `count` record
    /// prefix and the offset just past it.
    fn prefix(
        &mut self,
        count: usize,
        mut next: impl FnMut(usize, &T) -> Option<(T, usize)>,
    ) -> Option<(Vec<T>, usize)> {
        if count == 0 {
            return None; // the candidate scans filter zero counts out
        }
        while !self.dead && self.items.len() < count {
            let after = *self.ends.last().unwrap();
            match next(after, self.items.last().unwrap()) {
                Some((item, end)) => {
                    self.items.push(item);
                    self.ends.push(end);
                }
                None => self.dead = true,
            }
        }
        (self.items.len() >= count).then(|| (self.items[..count].to_vec(), self.ends[count - 1]))
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
fn bus_table_candidates<'a>(
    b: &'a [u8],
    runs: &'a RefCell<HashMap<usize, (Run<Bus>, u32)>>,
) -> impl Iterator<Item = (Vec<Bus>, usize)> + 'a {
    let limit = b.len().saturating_sub(4).min(0x10000);
    (0x20..limit).flat_map(move |at| {
        let count = u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) as usize;
        // Table glue between the count and the first record varies by a few
        // bytes per table and vintage; scan a small window for the record.
        let glues = (count != 0 && count <= 2_000_000).then_some(0..=48);
        glues
            .into_iter()
            .flatten()
            .filter_map(move |glue| bus_run(b, runs, at + 4 + glue, count))
    })
}

/// The bus record run from `first`, extended to `count` records if the bytes
/// allow. The run remembers the first record's family: one file's bus table
/// never mixes families, so the scan for each next record skips heads of the
/// other family (see [`bus_family`]).
fn bus_run(
    b: &[u8],
    runs: &RefCell<HashMap<usize, (Run<Bus>, u32)>>,
    first: usize,
    count: usize,
) -> Option<(Vec<Bus>, usize)> {
    let mut map = runs.borrow_mut();
    let (run, family) = match map.entry(first) {
        Entry::Occupied(e) => e.into_mut(),
        // A failed head parse is not cached: the table search probes far
        // more offsets than it accepts, and the probe itself is cheaper
        // than a map entry.
        Entry::Vacant(e) => {
            let (head, end) = read_bus_head(b, first).ok()?;
            let family = bus_family(head.unk);
            e.insert((Run::start(head.bus, end), family))
        }
    };
    let family = *family;
    run.prefix(count, |after, _| {
        // The record tail (undecoded; longer when flag bit 4 inserts a count
        // prefixed list) separates this record from the next; find the next
        // head by bounded scan.
        (after..after + RESYNC_WINDOW).find_map(|p| {
            read_bus_head(b, p)
                .ok()
                .filter(|(h, _)| bus_family(h.unk) == family)
                .map(|(h, end)| (h.bus, end))
        })
    })
}

/// Parse one bus record head at `at`; everything through the voltage angle
/// (the head layout both record families share). Returns the parsed bus and
/// leaves undecoded tail bytes, including the bit 4 list, to the resync.
fn read_bus_head(b: &[u8], at: usize) -> Probe<(BusHead, usize)> {
    let mut c = Cur { b, pos: at };
    let num = c.u32()? as usize;
    if num == 0 || num > 99_999_999 {
        return Err("implausible bus number");
    }
    let name = c.string(64)?;
    if name.is_empty() {
        return Err("empty bus name");
    }
    let unk = c.u32()?;
    if !known_bus_flags(unk) {
        return Err("bus record flags not in the validated set");
    }
    if unk & 1 == 0 {
        let _extra = c.u16()?;
    }
    let kv = c.f32()?;
    if !kv.is_finite() || !(0.0..=10_000.0).contains(&kv) {
        return Err("implausible nominal kV");
    }
    let area = c.u32()? as usize;
    let zone = c.u32()? as usize;
    let ba = c.u32()?;
    if area > 100_000_000 || zone > 100_000_000 || ba > 100_000_000 {
        return Err("implausible area/zone/BA number");
    }
    let _label = c.string(64)?;
    let vm = c.f64()?;
    let va_rad = c.f64()?;
    if !vm.is_finite() || !(0.0..=10.0).contains(&vm) || !va_rad.is_finite() || va_rad.abs() > 100.0
    {
        return Err("implausible voltage solution");
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
        name: Some(String::from_utf8_lossy(name).into_owned()),
        extras: Extras::new(),
    };
    Ok((BusHead { bus, unk }, c.pos))
}

// ---- Bus + ShortString ID tables (loads, generators, shunts) ----------------

/// One record of a device table keyed by bus + ShortString ID. `read` parses
/// the record head at the cursor (bus and ID already consumed) and returns
/// the element.
type ReadDevice<T> = fn(&mut Cur, BusId, &[u8]) -> Probe<T>;

fn read_device_head<T>(
    b: &[u8],
    at: usize,
    bus_ids: &BusIdSet,
    read: ReadDevice<T>,
) -> Probe<(T, usize)> {
    let mut c = Cur { b, pos: at };
    let bus = c.u32()? as usize;
    if !bus_ids.contains(bus) {
        return Err("record references an unknown bus");
    }
    let id = c.short_string(8)?;
    if id.is_empty() {
        return Err("empty device ID");
    }
    let v = read(&mut c, BusId(bus), id)?;
    Ok((v, c.pos))
}

/// Candidates for a count prefixed device table after `from`: every
/// `(count, glue)` whose full record walk succeeds, in scan order. The caller
/// keeps a candidate only if the tables that must follow it parse too.
fn device_table_candidates<'a, T: Clone + 'a>(
    b: &'a [u8],
    from: usize,
    bus_ids: &'a BusIdSet,
    read: ReadDevice<T>,
    runs: &'a RefCell<HashMap<usize, Run<T>>>,
) -> impl Iterator<Item = (Vec<T>, usize)> + 'a {
    let limit = (from + RESYNC_WINDOW).min(b.len().saturating_sub(4));
    (from..limit).flat_map(move |at| {
        let count = u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) as usize;
        let glues = (count != 0 && count <= 10_000_000).then_some(0..=48);
        glues
            .into_iter()
            .flatten()
            .filter_map(move |glue| device_run(b, runs, at + 4 + glue, count, bus_ids, read))
    })
}

/// The device record run from `first`, extended to `count` records if the
/// bytes allow (see [`Run`]).
fn device_run<T: Clone>(
    b: &[u8],
    runs: &RefCell<HashMap<usize, Run<T>>>,
    first: usize,
    count: usize,
    bus_ids: &BusIdSet,
    read: ReadDevice<T>,
) -> Option<(Vec<T>, usize)> {
    let mut map = runs.borrow_mut();
    let run = match map.entry(first) {
        Entry::Occupied(e) => e.into_mut(),
        // A failed head parse is not cached, as in the sibling run lookups.
        Entry::Vacant(e) => {
            let (item, end) = read_device_head(b, first, bus_ids, read).ok()?;
            e.insert(Run::start(item, end))
        }
    };
    run.prefix(count, |after, _| {
        // The undecoded record tail separates this record from the next.
        (after..after + RESYNC_WINDOW).find_map(|p| read_device_head(b, p, bus_ids, read).ok())
    })
}

/// Load record: one undecoded byte, then constant power P and Q in per unit
/// (f32). The byte is 0x00 in every 425 era record and 0x01 in every 483
/// era one while both auxes say every load is Closed, so it is not a status
/// byte; loads read as in service (see the module docs).
fn read_load(c: &mut Cur, bus: BusId, id: &[u8]) -> Probe<Load> {
    let _flag = c.u8()?;
    let p = c.f32()? * MVA_BASE;
    let q = c.f32()? * MVA_BASE;
    if !p.is_finite() || !q.is_finite() || p.abs() > 1.0e6 || q.abs() > 1.0e6 {
        return Err("implausible load power");
    }
    let mut extras = Extras::new();
    extras.insert(
        "LoadID".into(),
        serde_json::Value::String(String::from_utf8_lossy(id).into_owned()),
    );
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
fn read_gen(c: &mut Cur, bus: BusId, id: &[u8]) -> Probe<Generator> {
    let record_start = c.pos - (4 + 1) - id.len(); // u32 bus + the ID length byte
    let mut chosen = None;
    // +12 extends the observed set to two character IDs in a 2018 era
    // export (unobserved, but the pre-rework probe covered it).
    for anchor in [9usize, 10, 11, 12] {
        let mut probe = Cur {
            b: c.b,
            pos: record_start + anchor,
        };
        let Ok(vals) = (0..8).map(|_| probe.f32()).collect::<Probe<Vec<_>>>() else {
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
        return Err("generator record does not match the validated layouts");
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
fn read_shunt(c: &mut Cur, bus: BusId, id: &[u8]) -> Probe<Shunt> {
    let record_start = c.pos - (4 + 1) - id.len(); // u32 bus + the ID length byte
    let mut probe = Cur {
        b: c.b,
        pos: record_start + 24,
    };
    let b_mvar = probe.f32()? * MVA_BASE;
    if !b_mvar.is_finite() || b_mvar.abs() > 1.0e6 {
        return Err("implausible shunt MVAr");
    }
    c.pos = probe.pos;
    let mut extras = Extras::new();
    extras.insert(
        "ShuntID".into(),
        serde_json::Value::String(String::from_utf8_lossy(id).into_owned()),
    );
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

/// Locate and walk the branch table after `from`: the first `(count, glue)`
/// candidate whose walk succeeds and after which no further branch record
/// follows (a forged count word inside the glue can parse a prefix of the
/// real table; the true count lands where no further record follows).
fn find_branch_table(
    b: &[u8],
    from: usize,
    bus_ids: &BusIdSet,
    runs: &RefCell<HashMap<usize, Run<(Branch, u16)>>>,
) -> Option<Vec<Branch>> {
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
        let Some(first) =
            (at + 4..(at + 64).min(b.len())).find(|&p| read_branch_head(b, p, bus_ids).is_ok())
        else {
            continue;
        };
        if let Some((branches, after)) = branch_run(b, runs, first, count, bus_ids) {
            let continues =
                (after..after + RESYNC_WINDOW).any(|p| read_branch_head(b, p, bus_ids).is_ok());
            if !continues {
                return Some(branches.into_iter().map(|(br, _)| br).collect());
            }
        }
    }
    None
}

/// The branch record run from `first`, extended to `count` records if the
/// bytes allow (see [`Run`]). The items keep their flag words: the scan
/// window for the next record depends on the preceding record's bit 4.
fn branch_run(
    b: &[u8],
    runs: &RefCell<HashMap<usize, Run<(Branch, u16)>>>,
    first: usize,
    count: usize,
    bus_ids: &BusIdSet,
) -> Option<(Vec<(Branch, u16)>, usize)> {
    let mut map = runs.borrow_mut();
    let run = match map.entry(first) {
        Entry::Occupied(e) => e.into_mut(),
        // A failed head parse is not cached, as in the sibling run lookups.
        Entry::Vacant(e) => {
            let (br, end, flags) = read_branch_head(b, first, bus_ids).ok()?;
            e.insert(Run::start((br, flags), end))
        }
    };
    run.prefix(count, |after, prev| {
        // Flag bit 4 appends a variable structure to the record tail; the
        // 2019+ era writers store per bus f64 vectors and contingency label
        // text there, hundreds of KiB on some records, so the bounded
        // window cannot cover it. The scan extends to the buffer end for
        // those records only; the ~90 byte structural gauntlet of
        // read_branch_head keeps blob content from forging a record.
        let window_end = if prev.1 & 0x10 != 0 {
            b.len()
        } else {
            after + RESYNC_WINDOW
        };
        (after..window_end).find_map(|p| {
            read_branch_head(b, p, bus_ids)
                .ok()
                .map(|(br, end, flags)| ((br, flags), end))
        })
    })
}

/// Branch record, validated field by field against the aux siblings of all
/// three cases (6,491 records). After the impedances: two or three inline
/// per unit rating slots (by flag bit 1), a constant u32 tag, eleven f32
/// slots (zero in every available case), one zero byte, then the kind byte
/// that separates lines from transformers (which carry their tap next). The
/// tag and the zero byte are structural anchors: an unobserved variant
/// shifts them and dies loudly instead of misreading.
#[allow(clippy::many_single_char_names)] // r, x, b are the domain names
fn read_branch_head(b: &[u8], at: usize, bus_ids: &BusIdSet) -> Probe<(Branch, usize, u16)> {
    let mut c = Cur { b, pos: at };
    let from = c.u32()? as usize;
    let to = c.u32()? as usize;
    if !bus_ids.contains(from) || !bus_ids.contains(to) || from == to {
        return Err("branch references unknown buses");
    }
    let flags = c.u16()?;
    if !known_branch_flags(flags) {
        return Err("branch record flags not in the validated set");
    }
    let circuit = if flags & 1 == 0 {
        // The circuit ID is a Delphi ShortString[2]: one length byte and a
        // fixed two byte text area (a one character ID leaves the second
        // byte unused). Establishing this took the v19 file's parallel
        // circuit records, the first in the corpus with two character IDs.
        let n = c.u8()? as usize;
        if n == 0 || n > 2 {
            return Err("circuit ID length not 1 or 2");
        }
        let text = c.take(2)?;
        if !printable(&text[..n]) {
            return Err("circuit ID has non printable bytes");
        }
        Some(&text[..n])
    } else {
        // Omitted circuit: PowerWorld's default, observed as " 1" in the
        // sibling aux.
        None
    };
    let r = c.f32()?;
    let x = c.f32()?;
    let b_chg = c.f32()?;
    for v in [r, x, b_chg] {
        if !v.is_finite() || v.abs() > 1.0e4 {
            return Err("implausible branch impedance");
        }
    }
    let _g = c.f32()?;
    let inline = if flags & 2 == 0 { 3 } else { 2 };
    let mut rates = [0.0f64; 3];
    for slot in rates.iter_mut().take(inline) {
        let v = c.f32()?;
        if !v.is_finite() || !(0.0..=1.0e6).contains(&v) {
            return Err("implausible branch rating");
        }
        *slot = v * MVA_BASE;
    }
    let tag = c.u32()?;
    if tag != 12 {
        return Err("branch rating block tag not 12; unvalidated .pwb branch variant");
    }
    for _ in 0..11 {
        let v = c.f32()?;
        if !v.is_finite() || v.abs() > 1.0e6 {
            return Err("implausible branch rating block value");
        }
    }
    if c.u8()? != 0 {
        return Err("branch record separator byte not zero");
    }
    let kind = c.u8()?;
    let (device, tap) = match kind {
        0x01 => ("Line", 0.0),
        0x00 => {
            let tap = c.f32()?;
            if !tap.is_finite() || !(0.2..=5.0).contains(&tap) {
                return Err("implausible transformer tap");
            }
            ("Transformer", tap)
        }
        _ => {
            return Err("branch kind marker not in the validated set");
        }
    };
    let mut extras = Extras::new();
    extras.insert(
        LINE_CIRCUIT.into(),
        serde_json::Value::String(circuit.map_or_else(
            || " 1".to_string(),
            |s| String::from_utf8_lossy(s).into_owned(),
        )),
    );
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
    Ok((br, c.pos, flags))
}
