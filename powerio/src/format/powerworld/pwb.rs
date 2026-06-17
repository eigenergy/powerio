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
//! Supported header constants: 338, 368, 425, 483, 508, 537, 550, 551, and 554.
//! These constants gate only the writer era; a recognized constant still has
//! to pass the table walk. Constants 338/368/425 use the older generator record
//! (`bus`, ID, f32 block), 483/537/550/551 use the regulated bus record, 508
//! has been observed with both generator families, and 554 uses the regulated
//! record without the 2021 era presence byte. The bus, load, shunt, and branch
//! heads are more general: their flag words are Delphi field presence bitmasks,
//! so one decoded head model admits the observed 0x06, 0x26, and 0x66 families
//! as long as the later table walk still validates.
//!
//! The emerging structure is useful but bounded: the file is a sequence of
//! count-word tables separated by writer metadata, each record starts with a
//! small stable head, optional fields are controlled by bitmasks or short kind
//! markers, and long tails are skipped only after anchors prove the record kind.
//! That gives a general path for new vintages without guessing at fields.
//!
//! To add a new vintage, start with the smallest stable facts: header words,
//! bus flag census, table count positions, record anchors, and companion
//! export parity. Prefer widening a presence bit or table glue window only
//! after a full record walk still validates every later table. A new layout
//! belongs behind its own probe until a sibling `.aux`, `.raw`, `.epc`, or
//! `.m` file proves that it shares an existing record family.
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
//! - Status bytes: the 483 era generator record is the one located,
//!   validated status in the corpus (bit 0 of the byte one past the f32
//!   block, proven against the 94 open machines in the Texas7k aux). Every
//!   other device in every available case is in service, so no other out of
//!   service encoding is validated and those devices read as in service.
//!   The load record's post ID byte, once treated as a status, is 0x00 in
//!   the 425 era files and 0x01 in the 2021 era ones with every load Closed
//!   in both, so it is no status byte; the 425 era generator, the shunt,
//!   and the branch status bytes are unlocated.
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
//! - Bus voltage limits are not decoded; buses read with the 1.1/0.9
//!   defaults the aux reader also falls back to when the per rating set
//!   fields are absent.
//! - Branch angle limits have no PowerWorld field at all; branches read
//!   with the +-360 degree placeholder every reader uses for absence.

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
    let header_constant = expect_header(bytes)?;
    reject_unsupported_vintage(bytes)?;
    // The header constant pins the generator record layout wherever the
    // corpus is unambiguous: every 425 file carries the bus + ID shape and
    // every 483/537/550/551 file the regulated bus shape, while 508 saves
    // exist with both (Hawaii40 against the Texas7k v21 resave), so only
    // they try the two in sequence. Beyond pricing, this keeps the layout
    // a file cannot carry from ever outbidding the right one in the chain
    // search; a hypothetical file mixing eras fails loudly instead.
    let gen_variants = match header_constant {
        338 | 368 | 425 => GenVariants {
            plain: true,
            reg: false,
            simple_reg: false,
        },
        508 => GenVariants {
            plain: true,
            reg: true,
            simple_reg: false,
        },
        554 => GenVariants {
            plain: false,
            reg: false,
            simple_reg: true,
        },
        _ => GenVariants {
            plain: false,
            reg: true,
            simple_reg: false,
        },
    };
    let branch_count_can_include_trailer = header_constant == 554;
    // The narrow bus glue window prices the common files (see
    // bus_table_candidates); the wide retry exists so a small node level
    // resave (a bus table under 256 records with the v21 writer's 52 byte
    // glue) is a second slower search instead of a coverage cliff. The
    // retry only runs on files the narrow search already failed, enumerates
    // only the glue combinations the narrow pass could not reach, and
    // shares the bus run cache so nothing is walked twice.
    let bus_runs = RefCell::new(HashMap::new());
    match search_table_chain(
        bytes,
        name_hint,
        gen_variants,
        branch_count_can_include_trailer,
        &bus_runs,
        false,
    ) {
        Some(net) => net,
        None => search_table_chain(
            bytes,
            name_hint,
            gen_variants,
            branch_count_can_include_trailer,
            &bus_runs,
            true,
        )
        .unwrap_or_else(|| {
            Err(Error::FormatRead {
                format: FMT,
                message: "no table chain matches the validated .pwb layouts \
                                  (buses, loads, generators, shunts, branches in sequence)"
                    .into(),
            })
        }),
    }
}

/// Which generator record layouts the header constant admits (see
/// [`parse_pwb`]): the 425/508 era bus + ID shape (`plain`), the 2021 era
/// regulated bus shape (`reg`, [`read_gen_reg_record`]), and the 554 shape
/// whose regulated bus record omits the presence byte (`simple_reg`,
/// [`read_gen_reg_simple_record`]).
#[derive(Clone, Copy)]
struct GenVariants {
    plain: bool,
    reg: bool,
    simple_reg: bool,
}

/// One full depth first search for the table chain; `None` when no chain
/// matches. `wide_bus_glue` lifts the bus table's count gated glue window
/// (see [`bus_table_candidates`]) for the retry pass.
#[expect(clippy::too_many_lines)]
fn search_table_chain(
    bytes: &[u8],
    name_hint: Option<&str>,
    gen_variants: GenVariants,
    branch_count_can_include_trailer: bool,
    bus_runs: &RefCell<BusRuns>,
    wide_bus_glue: bool,
) -> Option<Result<Network>> {
    // A count word can be forged by record interiors and the case
    // description, so table location is a depth first search: a candidate at
    // any stage is kept only if every later table parses behind it, and the
    // first full chain wins. Wrong candidates die fast on their bounded
    // windows; a file with no valid chain fails loudly. The run caches make
    // the backtracking affordable: candidates pointing at the same first
    // record share one walk however many count words and search retries
    // reach it.
    for (buses, bus_shunts, bus_end, last_bus_unk) in
        bus_table_candidates(bytes, bus_runs, wide_bus_glue)
    {
        let Some(bus_ids) = BusIdSet::new(&buses) else {
            continue; // duplicate ids: not a real bus table
        };
        let mut best = None;
        let bus_names = bus_name_map(&buses);
        // The device and branch runs validate bus references, so their
        // caches are scoped to one bus table candidate.
        let load_runs = RefCell::new(HashMap::new());
        let gen_runs = RefCell::new(HashMap::new());
        let gen_reg_runs = RefCell::new(HashMap::new());
        let gen_reg_simple_runs = RefCell::new(HashMap::new());
        let shunt_runs = RefCell::new(HashMap::new());
        let branch_runs = RefCell::new(HashMap::new());
        // The load table's count word sits past the final bus record's
        // undecoded tail, which a bit 4 list can stretch beyond one window
        // (the 2030 build's lists run 1341 bytes); the seam scan honors it
        // exactly as the intra table stepping does.
        let load_scan_end = resync_end(bytes, bus_end, last_bus_unk & 0x10 != 0);
        for (loads, l_end) in device_table_candidates(
            bytes,
            bus_end..load_scan_end,
            &bus_ids,
            read_load_record,
            &load_runs,
            128,
            12,
        ) {
            // The generator table reads through the record layouts the
            // header constant admits (see parse_pwb). A file's table uses
            // exactly one; each gets its own run cache and the structural
            // gauntlets keep the wrong one from parsing. The newer layout's
            // table glue runs to 86 bytes in the v21 resave; the older
            // table glue reaches 104 bytes in the IEEE 24 bus save.
            let gen_candidates = gen_variants
                .plain
                .then(|| {
                    device_table_candidates(
                        bytes,
                        l_end..l_end.saturating_add(RESYNC_WINDOW),
                        &bus_ids,
                        read_gen_record,
                        &gen_runs,
                        128,
                        32,
                    )
                })
                .into_iter()
                .flatten()
                .chain(
                    gen_variants
                        .reg
                        .then(|| {
                            device_table_candidates(
                                bytes,
                                l_end..l_end.saturating_add(RESYNC_WINDOW),
                                &bus_ids,
                                read_gen_reg_record,
                                &gen_reg_runs,
                                128,
                                40,
                            )
                        })
                        .into_iter()
                        .flatten(),
                )
                .chain(
                    gen_variants
                        .simple_reg
                        .then(|| {
                            device_table_candidates(
                                bytes,
                                l_end..l_end.saturating_add(RESYNC_WINDOW),
                                &bus_ids,
                                read_gen_reg_simple_record,
                                &gen_reg_simple_runs,
                                128,
                                40,
                            )
                        })
                        .into_iter()
                        .flatten(),
                );
            for (generators, g_end) in gen_candidates {
                if gen_table_continues(bytes, g_end, &bus_ids, gen_variants) {
                    continue;
                }
                if !bus_shunts.is_empty() {
                    if let Some(branches) = find_branch_table(
                        bytes,
                        g_end,
                        &bus_ids,
                        &bus_names,
                        &branch_runs,
                        branch_count_can_include_trailer,
                    ) {
                        keep_best_chain(
                            &mut best,
                            chain_score(&loads, &bus_shunts, &branches, &generators),
                            checked_network(
                                name_hint,
                                buses.clone(),
                                loads.clone(),
                                bus_shunts.clone(),
                                branches,
                                generators.clone(),
                            ),
                        );
                    }
                }
                for (shunts, s_end) in device_table_candidates(
                    bytes,
                    g_end..g_end.saturating_add(RESYNC_WINDOW),
                    &bus_ids,
                    read_shunt_record,
                    &shunt_runs,
                    48,
                    28,
                ) {
                    let Some(branches) = find_branch_table(
                        bytes,
                        s_end,
                        &bus_ids,
                        &bus_names,
                        &branch_runs,
                        branch_count_can_include_trailer,
                    ) else {
                        continue;
                    };
                    let mut shunts = shunts;
                    extend_unique_shunts(&mut shunts, &bus_shunts);
                    keep_best_chain(
                        &mut best,
                        chain_score(&loads, &shunts, &branches, &generators),
                        checked_network(
                            name_hint,
                            buses.clone(),
                            loads.clone(),
                            shunts,
                            branches,
                            generators.clone(),
                        ),
                    );
                }
                if let Some(branches) = find_branch_table(
                    bytes,
                    g_end,
                    &bus_ids,
                    &bus_names,
                    &branch_runs,
                    branch_count_can_include_trailer,
                ) {
                    keep_best_chain(
                        &mut best,
                        chain_score(&loads, &bus_shunts, &branches, &generators),
                        checked_network(
                            name_hint,
                            buses.clone(),
                            loads.clone(),
                            bus_shunts.clone(),
                            branches,
                            generators.clone(),
                        ),
                    );
                }
            }
        }
        if let Some((_, net)) = best {
            return Some(net);
        }
    }
    None
}

/// Keep the table chain with the largest decoded electrical core.
fn keep_best_chain(
    best: &mut Option<(usize, Result<Network>)>,
    score: usize,
    net: Result<Network>,
) {
    let candidate_ok = net.is_ok();
    let replace = match best.as_ref() {
        None => true,
        Some((best_score, best_net)) => match (best_net.is_ok(), candidate_ok) {
            (false, true) => true,
            (true, false) => false,
            _ => score > *best_score,
        },
    };
    if replace {
        *best = Some((score, net));
    }
}

/// Score a candidate table chain by decoded element count.
fn chain_score(
    loads: &[Load],
    shunts: &[Shunt],
    branches: &[Branch],
    generators: &[Generator],
) -> usize {
    loads.len() + shunts.len() + branches.len() + generators.len()
}

/// Add bus tail shunts without duplicating the dedicated shunt table rows.
fn extend_unique_shunts(shunts: &mut Vec<Shunt>, extra: &[Shunt]) {
    for shunt in extra {
        if !shunts.iter().any(|existing| {
            existing.bus == shunt.bus
                && (existing.g - shunt.g).abs() <= 1e-9
                && (existing.b - shunt.b).abs() <= 1e-9
        }) {
            shunts.push(shunt.clone());
        }
    }
}

/// Check whether another generator record starts soon after a candidate table.
///
/// This rejects short prefixes when a wrong count word points into the real
/// generator table.
fn gen_table_continues(
    bytes: &[u8],
    after: usize,
    bus_ids: &BusIdSet,
    variants: GenVariants,
) -> bool {
    (after..after.saturating_add(RESYNC_WINDOW).min(bytes.len())).any(|p| {
        (variants.plain && read_gen_record(bytes, p, bus_ids).is_ok())
            || (variants.reg && read_gen_reg_record(bytes, p, bus_ids).is_ok())
            || (variants.simple_reg && read_gen_reg_simple_record(bytes, p, bus_ids).is_ok())
    })
}

/// Assemble the decoded tables and run the common reference checks.
fn checked_network(
    name_hint: Option<&str>,
    mut buses: Vec<Bus>,
    loads: Vec<Load>,
    shunts: Vec<Shunt>,
    branches: Vec<Branch>,
    generators: Vec<Generator>,
) -> Result<Network> {
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
    net.check_references(FMT).map(|()| net)
}

// ---- Cursor -----------------------------------------------------------------

/// Bounds checked cursor for little endian record probes.
struct Cur<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Cur<'a> {
    /// Take `n` bytes and advance the cursor.
    fn take(&mut self, n: usize) -> Probe<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or("truncated record")?;
        let s = self.b.get(self.pos..end).ok_or("truncated record")?;
        self.pos = end;
        Ok(s)
    }

    /// Read one byte.
    fn u8(&mut self) -> Probe<u8> {
        Ok(self.take(1)?[0])
    }
    /// Read a little endian u16.
    fn u16(&mut self) -> Probe<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    /// Read a little endian u32.
    fn u32(&mut self) -> Probe<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    /// Read a little endian f32 and widen to f64.
    fn f32(&mut self) -> Probe<f64> {
        Ok(f64::from(f32::from_le_bytes(
            self.take(4)?.try_into().unwrap(),
        )))
    }
    /// Read a little endian f64.
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

    /// A fixed capacity Delphi `string[2]`: one length byte plus a fixed two
    /// byte text area (a one character value leaves the second byte unused).
    /// Branch circuit IDs and generator IDs are stored this way; the fixed
    /// capacity was established by the v19 file's parallel circuit records.
    fn short_string_2(&mut self) -> Probe<&'a [u8]> {
        let n = self.u8()? as usize;
        if n == 0 || n > 2 {
            return Err("fixed capacity ID length not 1 or 2");
        }
        let text = self.take(2)?;
        if !printable(&text[..n]) {
            return Err("fixed capacity ID has non printable bytes");
        }
        Ok(&text[..n])
    }
}

/// How far a bit 4 record's tail blob may push the next record: the largest
/// observed blob is 406 KiB (an ACTIVSg500 branch record, see
/// docs/powerworld.md), so four MiB is an order of magnitude of headroom
/// while bounding what a crafted file can make the scan walk per record.
const BLOB_WINDOW: usize = 4 << 20;

/// How far the scan for the next record may look past `after`: one bounded
/// window normally, the blob window when the preceding record's flag bit 4
/// inserted a count prefixed list (the 2019+ era branch blobs run to 406 KiB
/// and the 2030 build's bus lists past one window; the record head gauntlets
/// keep blob bytes from forging a record).
fn resync_end(b: &[u8], after: usize, prev_bit4: bool) -> usize {
    if prev_bit4 {
        after.saturating_add(BLOB_WINDOW).min(b.len())
    } else {
        after.saturating_add(RESYNC_WINDOW).min(b.len())
    }
}

/// True when a probed string is printable ASCII.
fn printable(s: &[u8]) -> bool {
    s.iter().all(|&c| (0x20..0x7f).contains(&c))
}

/// Borrow a bounded byte slice at an absolute offset.
fn slice_at(b: &[u8], at: usize, n: usize) -> Option<&[u8]> {
    at.checked_add(n).and_then(|end| b.get(at..end))
}

/// Add an absolute offset without wrapping.
fn checked_offset(at: usize, add: usize) -> Probe<usize> {
    at.checked_add(add).ok_or("truncated record")
}

/// Reject impossible count words before walking a table.
fn count_fits(b: &[u8], first: usize, count: usize, min_record_len: usize) -> bool {
    let Some(remaining) = b.len().checked_sub(first) else {
        return false;
    };
    count
        .checked_mul(min_record_len)
        .is_some_and(|min_bytes| min_bytes <= remaining)
}

/// Read a little endian u32 at an absolute offset.
fn u32_at(b: &[u8], at: usize) -> Probe<u32> {
    slice_at(b, at, 4)
        .and_then(|s| <[u8; 4]>::try_from(s).ok())
        .map(u32::from_le_bytes)
        .ok_or("truncated record")
}

/// Read a little endian f32 at an absolute offset and widen to f64.
fn f32_at(b: &[u8], at: usize) -> Probe<f64> {
    slice_at(b, at, 4)
        .and_then(|s| <[u8; 4]>::try_from(s).ok())
        .map(f32::from_le_bytes)
        .map(f64::from)
        .ok_or("truncated record")
}

/// Read a length prefixed printable ASCII string at an absolute offset.
fn string_at(b: &[u8], at: usize, max: usize) -> Probe<String> {
    let n = u32_at(b, at)? as usize;
    if n > max {
        return Err("string length exceeds the field maximum");
    }
    let s = slice_at(b, checked_offset(at, 4)?, n).ok_or("truncated record")?;
    if !printable(s) {
        return Err("string has non printable bytes");
    }
    Ok(String::from_utf8_lossy(s).into_owned())
}

/// Validate the file head and return the writer format constant (the u64 at
/// offset 0x08) for the layout keying in [`parse_pwb`].
fn expect_header(b: &[u8]) -> Result<u64> {
    const DECODED: [u64; 9] = [338, 368, 425, 483, 508, 537, 550, 551, 554];
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
    // Every known PowerWorld binary starts with 15000. The next two words
    // identify the writer family: the decoded constants cover older 0x06 bus
    // records (338/368), the Simulator 19/20/current 425 family, the 2021
    // regulated generator family (483/537/550/551), the mixed 508 saves, and
    // the 554 regulated generator variant. Header admission is not trust:
    // every table still has to pass the record probes below.
    if c != 20 || !DECODED.contains(&v) {
        return Err(unsupported_vintage(format!(
            "header format words ({v}, {c}); the decoded eras are \
             338/368/425/483/508/537/550/551/554 with 20"
        )));
    }
    Ok(v)
}

/// Reject files whose leading 64 KiB carries no run of validated bus record
/// heads before the table search reaches a generic "no chain" error. The
/// decoded bus head families share enough structure that fewer than two
/// validated heads in this window means an unrecognized body layout, not a
/// sparse case.
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
            "unsupported PowerWorld .pwb vintage: {detail}; only the validated \
             338/368/425/483/508/537/550/551/554 layouts are decoded (see docs/powerworld.md)"
        ),
    }
}

// ---- Search machinery --------------------------------------------------------

/// Bus id membership for the record probes, the hottest check in the table
/// search (every probed byte offset starts with one or two lookups). A
/// bitmap over the id range replaces hashing; [`read_bus_head`] caps ids at
/// 99,999,999 and the corpus tops out around 790,000, but a forged
/// candidate can pair a tiny count with an id near the cap, so tables whose
/// id range dwarfs their count fall back to a sorted list instead of
/// allocating megabytes per forged candidate.
enum BusIdSet {
    Bitmap(Vec<u64>),
    Sparse(Vec<usize>),
}

impl BusIdSet {
    /// `None` when an id repeats: a table with duplicate bus numbers is a
    /// forged candidate, not a real bus table.
    fn new(buses: &[Bus]) -> Option<Self> {
        let max = buses.iter().map(|b| b.id.0).max().unwrap_or(0);
        let words = max / 64 + 1;
        if words > (buses.len() * 4).max(1024) {
            let mut ids: Vec<usize> = buses.iter().map(|b| b.id.0).collect();
            ids.sort_unstable();
            if ids.windows(2).any(|w| w[0] == w[1]) {
                return None;
            }
            return Some(Self::Sparse(ids));
        }
        let mut bits = vec![0u64; words];
        for bus in buses {
            let (w, bit) = (bus.id.0 / 64, 1u64 << (bus.id.0 % 64));
            if bits[w] & bit != 0 {
                return None;
            }
            bits[w] |= bit;
        }
        Some(Self::Bitmap(bits))
    }

    /// Check whether a decoded bus id exists.
    #[inline]
    fn contains(&self, id: usize) -> bool {
        match self {
            Self::Bitmap(words) => words
                .get(id / 64)
                .is_some_and(|w| w & (1 << (id % 64)) != 0),
            Self::Sparse(ids) => ids.binary_search(&id).is_ok(),
        }
    }
}

/// Build an uppercase bus name index for records that point by name.
fn bus_name_map(buses: &[Bus]) -> HashMap<String, BusId> {
    buses
        .iter()
        .filter_map(|bus| {
            bus.name
                .as_ref()
                .map(|name| (name.trim().to_ascii_uppercase(), bus.id))
        })
        .collect()
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
    /// Start a record run with its first validated record.
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
    shunt: Option<Shunt>,
    /// The flags u32 between name and nominal kV: a Delphi field presence
    /// bitmask, not a per file constant. Bit 5 set marks the Simulator 20
    /// era record family (clear on the Simulator 19 era 0x06/0x07 family,
    /// whose tails are shorter), bit 4 set marks a count prefixed list in
    /// the record tail (2016/2017 era exports and the 2030 build), bit 0
    /// clear means one extra u16 sits before the nominal kV (observed on
    /// generator buses). The 2019+ era writers add bits 6 and 8, both per
    /// record (the v21 resave clears bit 6 on its slack bus record; the
    /// bit 6 tails carry a location string block), with their fields in
    /// the undecoded tail.
    unk: u32,
}

/// Whether a bus record flag word is one this reader decodes: base bits
/// `0x06` plus any combination of the observed presence bits. Bit 5 changes
/// the tail family (`0x06` vs `0x26` era), while bits 6, 8, 10, 12, and 13
/// were admitted only after full table walks showed they leave the decoded
/// head layout unchanged.
fn known_bus_flags(unk: u32) -> bool {
    unk & !0x3571 == 0x06
}

/// The record family bits of a bus flag word. One bus table cannot mix tail
/// families, but individual records can toggle optional presence bits inside
/// a family. Bit 5 stays in the family key because the 0x06 and 0x26 era tails
/// differ; the other admitted bits are per record fields or skipped tails.
fn bus_family(unk: u32) -> u32 {
    unk & !0x3551
}

/// The bus run cache: keyed by first record offset, each entry carrying the
/// walked `(bus, flag word)` records and the table's family bits.
type BusRunItem = (Bus, u32, Option<Shunt>);
type BusRuns = HashMap<usize, (Run<BusRunItem>, u32)>;

/// Bus table candidates: each `(count, glue)` position after the header whose
/// record walk succeeds, in scan order, yielding the records, the offset
/// past the last decoded head, and the last record's flag word (the load
/// table seam needs its bit 4). The caller validates each candidate by
/// parsing the tables that must follow it.
fn bus_table_candidates<'a>(
    b: &'a [u8],
    runs: &'a RefCell<BusRuns>,
    wide_glue: bool,
) -> impl Iterator<Item = (Vec<Bus>, Vec<Shunt>, usize, u32)> + 'a {
    let limit = b.len().saturating_sub(4).min(0x10000);
    (0x20..limit).flat_map(move |at| {
        let count = u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) as usize;
        // Table glue between the count and the first record varies by a few
        // bytes per table and vintage; scan a small window for the record.
        // The v21 resave's bus glue runs 52 bytes, past the 48 every other
        // export observes; the first search pass widens the window only for
        // large counts (every observed wide glue table is a node level
        // resave with thousands of buses, and forged count words are
        // overwhelmingly small values, so widening their window prices
        // every file). The retry pass covers exactly the complement (the
        // wide glues for small counts), so with the shared run cache the
        // two passes together cost one full sweep.
        let glues = if wide_glue {
            (count != 0 && count < 256).then_some(49..=96)
        } else {
            let max_glue = if count >= 256 { 96 } else { 48 };
            (count != 0 && count <= 2_000_000).then_some(0..=max_glue)
        };
        glues
            .into_iter()
            .flatten()
            .filter_map(move |glue| {
                let first = at.checked_add(4)?.checked_add(glue)?;
                count_fits(b, first, count, 32)
                    .then(|| bus_run(b, runs, first, count))
                    .flatten()
            })
            .map(|(heads, end)| {
                let last_unk = heads.last().map_or(0, |(_, unk, _)| *unk);
                let shunts = heads
                    .iter()
                    .filter_map(|(bus, _, shunt)| {
                        shunt.clone().map(|mut shunt| {
                            shunt.bus = bus.id;
                            shunt
                        })
                    })
                    .collect();
                (
                    heads.into_iter().map(|(bus, _, _)| bus).collect(),
                    shunts,
                    end,
                    last_unk,
                )
            })
    })
}

/// The bus record run from `first`, extended to `count` records if the bytes
/// allow. The run remembers the first record's family: one file's bus table
/// never mixes families, so the scan for each next record skips heads of the
/// other family (see [`bus_family`]). The items keep their flag words: the
/// scan window for the next record depends on the preceding record's bit 4,
/// as in the branch run.
fn bus_run(
    b: &[u8],
    runs: &RefCell<BusRuns>,
    first: usize,
    count: usize,
) -> Option<(Vec<BusRunItem>, usize)> {
    let mut map = runs.borrow_mut();
    let (run, family) = match map.entry(first) {
        Entry::Occupied(e) => e.into_mut(),
        // A failed head parse is not cached: the table search probes far
        // more offsets than it accepts, and the probe itself is cheaper
        // than a map entry.
        Entry::Vacant(e) => {
            let (head, end) = read_bus_head(b, first).ok()?;
            let family = bus_family(head.unk);
            e.insert((Run::start((head.bus, head.unk, head.shunt), end), family))
        }
    };
    let family = *family;
    run.prefix(count, |after, prev| {
        // The record tail (undecoded; longer when flag bit 4 inserts a
        // count prefixed list) separates this record from the next; find
        // the next head by bounded scan (see resync_end).
        (after..resync_end(b, after, prev.1 & 0x10 != 0)).find_map(|p| {
            read_bus_head(b, p)
                .ok()
                .filter(|(h, _)| bus_family(h.unk) == family)
                .map(|(h, end)| ((h.bus, h.unk, h.shunt), end))
        })
    })
}

/// Parse one bus record head at `at`; everything through the voltage angle.
/// Header 338 and some small header 425 saves omit the balancing authority
/// field between zone and label. Returns the parsed bus and leaves undecoded
/// tail bytes, including the bit 4 list, to the resync.
fn read_bus_head(b: &[u8], at: usize) -> Probe<(BusHead, usize)> {
    let mut c = Cur { b, pos: at };
    let num = c.u32()? as usize;
    if num == 0 || num > 99_999_999 {
        return Err("implausible bus number");
    }
    let name_len = c.u32()? as usize;
    if name_len == 0 {
        return Err("empty bus name");
    }
    if name_len > 64 {
        return Err("string length exceeds the field maximum");
    }
    let name = c.take(name_len)?;
    // The flag mask (a handful of admitted words out of 2^32) is far more
    // selective than the name text scan, so it gates first; the accept set
    // is unchanged, only the rejection order.
    let unk = c.u32()?;
    if !known_bus_flags(unk) {
        return Err("bus record flags not in the validated set");
    }
    if !printable(name) {
        return Err("string has non printable bytes");
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
    if area > 100_000_000 || zone > 100_000_000 {
        return Err("implausible area/zone/BA number");
    }
    let after_zone = c.pos;
    let mut with_ba = Cur { b, pos: after_zone };
    let with_ba_result = (|| -> Probe<(f64, f64)> {
        let ba = with_ba.u32()?;
        if ba > 100_000_000 {
            return Err("implausible area/zone/BA number");
        }
        read_bus_label_and_solution(&mut with_ba)
    })();
    let (vm, va_rad) = if let Ok(solution) = with_ba_result {
        c.pos = with_ba.pos;
        solution
    } else {
        let mut old = Cur { b, pos: after_zone };
        let solution = read_bus_label_and_solution(&mut old)?;
        c.pos = old.pos;
        solution
    };
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
    let shunt = bus_tail_shunt(b, c.pos, BusId(num));
    Ok((BusHead { bus, shunt, unk }, c.pos))
}

/// Decode the optional fixed shunt stored in some bus record tails.
fn bus_tail_shunt(b: &[u8], after_head: usize, bus: BusId) -> Option<Shunt> {
    let g_pu = b
        .get(after_head.checked_add(1)?..after_head.checked_add(5)?)
        .and_then(|s| <[u8; 4]>::try_from(s).ok())
        .map(f32::from_le_bytes)
        .map(f64::from)
        .filter(|g| g.is_finite() && g.abs() <= 1.0e6)
        .unwrap_or(0.0);
    let b_pu = b
        .get(after_head.checked_add(5)?..after_head.checked_add(9)?)
        .and_then(|s| <[u8; 4]>::try_from(s).ok())
        .map(f32::from_le_bytes)
        .map(f64::from)?;
    if !b_pu.is_finite() || b_pu.abs() > 1.0e6 || (g_pu.abs() <= 1e-9 && b_pu.abs() <= 1e-9) {
        return None;
    }
    let mut extras = Extras::new();
    extras.insert(
        "ShuntID".into(),
        serde_json::Value::String("BusShunt".into()),
    );
    Some(Shunt {
        bus,
        g: g_pu * MVA_BASE,
        b: b_pu * MVA_BASE,
        in_service: true,
        extras,
    })
}

/// Read the bus label plus solved voltage magnitude and angle.
fn read_bus_label_and_solution(c: &mut Cur<'_>) -> Probe<(f64, f64)> {
    let _label = c.string(64)?;
    let vm = c.f64()?;
    let va_rad = c.f64()?;
    Ok((vm, va_rad))
}

// ---- Device tables (loads, generators, shunts) -------------------------------

/// One whole device record: parse at `at`, return the element and the offset
/// just past the decoded head (undecoded tail bytes are the resync scan's to
/// skip). One function per validated record layout. The bound is generic
/// rather than a `fn` pointer so each table's probe monomorphizes and the
/// early rejection checks inline into the resync scans, the hottest loops
/// in the search.
trait ReadRecord<T>: Fn(&[u8], usize, &BusIdSet) -> Probe<(T, usize)> + Copy {}
impl<T, F: Fn(&[u8], usize, &BusIdSet) -> Probe<(T, usize)> + Copy> ReadRecord<T> for F {}

/// The bus + ShortString ID prefix the 425/508 era device records share.
/// `read` parses the rest of the record head at the cursor and returns the
/// element.
fn read_device_head<T>(
    b: &[u8],
    at: usize,
    bus_ids: &BusIdSet,
    read: fn(&mut Cur, BusId, &[u8]) -> Probe<T>,
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

/// Probe one load record using the shared device head.
fn read_load_record(b: &[u8], at: usize, bus_ids: &BusIdSet) -> Probe<(Load, usize)> {
    read_device_head(b, at, bus_ids, read_load)
}

/// Probe one plain generator record using the shared device head.
fn read_gen_record(b: &[u8], at: usize, bus_ids: &BusIdSet) -> Probe<(Generator, usize)> {
    read_device_head(b, at, bus_ids, read_gen)
}

/// Probe one switched shunt record using the shared device head.
fn read_shunt_record(b: &[u8], at: usize, bus_ids: &BusIdSet) -> Probe<(Shunt, usize)> {
    read_device_head(b, at, bus_ids, read_shunt)
}

/// Candidates for a count prefixed device table after `from`: every
/// `(count, glue)` whose full record walk succeeds, in scan order. The caller
/// keeps a candidate only if the tables that must follow it parse too.
fn device_table_candidates<'a, T: Clone + 'a>(
    b: &'a [u8],
    scan: std::ops::Range<usize>,
    bus_ids: &'a BusIdSet,
    read: impl ReadRecord<T> + 'a,
    runs: &'a RefCell<HashMap<usize, Run<T>>>,
    max_glue: usize,
    min_record_len: usize,
) -> impl Iterator<Item = (Vec<T>, usize)> + 'a {
    let limit = scan.end.min(b.len().saturating_sub(4));
    (scan.start..limit).flat_map(move |at| {
        let count = u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) as usize;
        let glues = (count != 0 && count <= 10_000_000).then_some(0..=max_glue);
        glues.into_iter().flatten().filter_map(move |glue| {
            let first = at.checked_add(4)?.checked_add(glue)?;
            count_fits(b, first, count, min_record_len)
                .then(|| device_run(b, runs, first, count, bus_ids, read))
                .flatten()
        })
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
    read: impl ReadRecord<T>,
) -> Option<(Vec<T>, usize)> {
    let mut map = runs.borrow_mut();
    let run = match map.entry(first) {
        Entry::Occupied(e) => e.into_mut(),
        // A failed head parse is not cached, as in the sibling run lookups.
        Entry::Vacant(e) => {
            let (item, end) = read(b, first, bus_ids).ok()?;
            e.insert(Run::start(item, end))
        }
    };
    run.prefix(count, |after, _| {
        // The undecoded record tail separates this record from the next.
        (after..after.saturating_add(RESYNC_WINDOW).min(b.len()))
            .find_map(|p| read(b, p, bus_ids).ok())
    })
}

/// Load record: one undecoded byte, then constant power P and Q in per unit
/// (f32). The byte is 0x00 in every 425 era record and 0x01 in every 483
/// era one while both auxes say every load is Closed, so it is not a status
/// byte; loads read as in service (see the module docs).
fn read_load(c: &mut Cur, bus: BusId, id: &[u8]) -> Probe<Load> {
    let record_start = c.pos - (4 + 1) - id.len(); // u32 bus + the ID length byte
    let flag = c.u8()?;
    if flag > 1 {
        return Err("load status byte not in the validated set");
    }
    let mut p = c.f32()? * MVA_BASE;
    let mut q = c.f32()? * MVA_BASE;
    let mut in_service = true;
    if flag == 0 && p.abs() < 1e-30 && q.abs() < 1e-30 {
        let early_p = f32_at(c.b, checked_offset(record_start, 25)?)? * MVA_BASE;
        let early_q = f32_at(c.b, checked_offset(record_start, 29)?)? * MVA_BASE;
        let late_p = f32_at(c.b, checked_offset(record_start, 33)?)? * MVA_BASE;
        let late_q = f32_at(c.b, checked_offset(record_start, 37)?)? * MVA_BASE;
        let early_is_marker = (early_p - MVA_BASE).abs() <= 1e-6 && early_q.abs() <= 1e-30;
        let (alt_p, alt_q, end) = if early_is_marker {
            (late_p, late_q, checked_offset(record_start, 41)?)
        } else {
            (early_p, early_q, checked_offset(record_start, 33)?)
        };
        if alt_p.abs() > 1e-30 || alt_q.abs() > 1e-30 {
            p = alt_p;
            q = alt_q;
            in_service = !early_is_marker;
        }
        c.pos = c.pos.max(end);
    }
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
        in_service,
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
            pos: checked_offset(record_start, anchor)?,
        };
        if let Ok(vals) = read_gen_f32_block(&mut probe) {
            chosen = Some((vals, probe.pos));
            break;
        }
    }
    let Some((v, end)) = chosen else {
        return Err("generator record does not match the validated layouts");
    };
    c.pos = end;
    // The status byte is unlocated within the flag bytes of this era's
    // record; every available machine is Closed (see the module docs).
    Ok(gen_from_block(bus, &v, true))
}

/// The eight consecutive f32 per unit values both generator record eras
/// share: MW setpoint, MVAr setpoint, MVRMax, MVRMin, GenVoltSet, GenMVABase,
/// MWMax, MWMin. The voltage setpoint and MVA base ranges anchor the layout;
/// a block that fails them is not a generator record.
fn read_gen_f32_block(c: &mut Cur) -> Probe<[f64; 8]> {
    let mut v = [0.0f64; 8];
    // Each slot checks as it reads, and the two anchor ranges right after
    // their slots, so a forged offset stops within a few reads instead of
    // always paying all eight; same predicates, same accept set.
    for (i, slot) in v.iter_mut().enumerate() {
        let x = c.f32()?;
        if !x.is_finite() || x.abs() >= 1.0e6 {
            return Err("generator record does not match the validated layouts");
        }
        if (i == 4 && !(0.5..=1.6).contains(&x)) || (i == 5 && !(0.1..=1.0e5).contains(&x)) {
            return Err("generator record does not match the validated layouts");
        }
        *slot = x;
    }
    Ok(v)
}

/// A [`Generator`] from the shared f32 block (see [`read_gen_f32_block`]).
fn gen_from_block(bus: BusId, v: &[f64; 8], in_service: bool) -> Generator {
    Generator {
        bus,
        pg: v[0] * MVA_BASE,
        qg: v[1] * MVA_BASE,
        qmax: v[2] * MVA_BASE,
        qmin: v[3] * MVA_BASE,
        vg: v[4],
        mbase: v[5],
        pmax: v[6] * MVA_BASE,
        pmin: v[7] * MVA_BASE,
        in_service,
        cost: None,
        caps: Default::default(),
    }
}

/// 2021 era generator record (header constant 483, the Texas7k export),
/// validated against all 731 machines of the same day aux: u32 terminal
/// bus, u32 regulated bus (the inserted field that distinguishes this
/// layout; on plants regulating a remote bus the two differ, which is what
/// made the older record model misread until the boundary was re-fit), a
/// fixed capacity ShortString[2] ID, a constant 0x01 byte, one undecoded
/// byte, then a presence byte whose bit 0 inserts an f32 and bit 1 one
/// byte, then the same eight f32 block as the older eras. One past the
/// block sit a zero byte and the status byte: bit 0 is the in service bit,
/// validated against the aux's 637 Closed and 94 Open machines (the
/// corpus's first out of service devices). The f32 after it reads as
/// GenRMPCT in the aux (100.0 on every record) and anchors the layout.
fn read_gen_reg_record(b: &[u8], at: usize, bus_ids: &BusIdSet) -> Probe<(Generator, usize)> {
    let mut c = Cur { b, pos: at };
    let bus = c.u32()? as usize;
    if !bus_ids.contains(bus) {
        return Err("record references an unknown bus");
    }
    let reg = c.u32()? as usize;
    if !bus_ids.contains(reg) {
        return Err("regulated bus is not a known bus");
    }
    let _id = c.short_string_2()?;
    if c.u8()? != 1 {
        return Err("generator record lead byte not 1");
    }
    let _ = c.u8()?; // varies per record (7 through 37 observed); undecoded
    // Presence byte: bit 0 inserts an f32, bit 1 one byte (both in the
    // 2021 export), bit 5 another f32 (the 2030 build); the eight f32
    // block follows whatever the bits insert.
    let pres = c.u8()?;
    if pres & !0x23 != 0 {
        return Err("generator presence byte not in the validated set");
    }
    if pres & 0x22 == 0x22 {
        // Bits 1 and 5 never co-occur in the corpus, so the order of their
        // inserted fields is unestablished; guessing it risks reading a
        // misaligned f32, so the combination rejects until a file shows it.
        return Err("generator presence bits 1 and 5 together are unobserved");
    }
    for bit in [0x01, 0x20] {
        if pres & bit != 0 {
            let v = c.f32()?;
            if !v.is_finite() || v.abs() > 1.0e6 {
                return Err("implausible presence gated generator value");
            }
        }
    }
    if pres & 2 != 0 {
        let _ = c.u8()?;
    }
    let v = read_gen_f32_block(&mut c)?;
    read_gen_reg_tail(&mut c, bus, &v)
}

/// Header 554 regulated generator record: terminal bus, regulated bus,
/// fixed capacity ID, two zero bytes, then the shared f32 block and the
/// same status/RMPCT tail as [`read_gen_reg_record`].
fn read_gen_reg_simple_record(
    b: &[u8],
    at: usize,
    bus_ids: &BusIdSet,
) -> Probe<(Generator, usize)> {
    let mut c = Cur { b, pos: at };
    let bus = c.u32()? as usize;
    if !bus_ids.contains(bus) {
        return Err("record references an unknown bus");
    }
    let reg = c.u32()? as usize;
    if !bus_ids.contains(reg) {
        return Err("regulated bus is not a known bus");
    }
    let _id = c.short_string_2()?;
    if c.u8()? != 0 || c.u8()? != 0 {
        return Err("generator record separator bytes not zero");
    }
    let v = read_gen_f32_block(&mut c)?;
    read_gen_reg_tail(&mut c, bus, &v)
}

/// Read the status and RMPCT tail shared by regulated generator records.
fn read_gen_reg_tail(c: &mut Cur<'_>, bus: usize, v: &[f64; 8]) -> Probe<(Generator, usize)> {
    if c.u8()? != 0 {
        return Err("generator record separator byte not zero");
    }
    let status = c.u8()?;
    if status & !0x01 != 0x08 {
        return Err("generator status byte not in the validated set");
    }
    let rmpct = c.f32()?;
    if !rmpct.is_finite() || !(0.0..=1000.0).contains(&rmpct) {
        return Err("implausible remote regulation percentage");
    }
    Ok((gen_from_block(BusId(bus), v, status & 1 == 1), c.pos))
}

/// Shunt record: nominal MVAr as f32 at +24 from the record start, validated
/// on all 199 shunts across the three sibling cases. The slot at +20 is 0.0
/// in the Simulator 20 era files but 0.99 in the 2016 export, so it is not
/// the nominal MW (see the module docs); shunts read with `g = 0`.
fn read_shunt(c: &mut Cur, bus: BusId, id: &[u8]) -> Probe<Shunt> {
    let record_start = c.pos - (4 + 1) - id.len(); // u32 bus + the ID length byte
    let mut probe = Cur {
        b: c.b,
        pos: checked_offset(record_start, 24)?,
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
/// `0x4C` plus any combination of bits 0, 1, 4, 5, and 7, a Delphi field
/// presence bitmask like the bus record's. Bit 0 set omits
/// the circuit ID string and its status byte (the PowerWorld default " 1"
/// applies), bit 1 set means two inline rating slots instead of three
/// (the Simulator 19 era writer inlines three), bit 4 marks a count
/// prefixed list in the record tail. Bit 7 is set on every 425/508 era
/// record; the 2021 era Texas7k exports clear it on most lines while
/// setting it on every transformer and a few dozen lines, with the head
/// layout through the kind byte identical either way (its field lives in
/// the undecoded tail). Admitting the bit 7 clear words doubles the flag
/// vocabulary; the measured cost on the 425 era corpus is a few
/// microseconds (benchmarks/RESULTS.md), and a mask keyed to the generator
/// layout was tried and rejected anyway, since the strict mask turns real
/// bit 7 clear records invisible to the table end check and a forged
/// short table can win.
/// Observed words: 0xEC/0xFC (2016), 0xEE/0xEF (2018 and v19), 0xFE/0xFF
/// (v19), 0x6C and 0xEC/0xED (Texas7k), 0xCE on the Australian series
/// capacitor records, and the same families with bits 10 or 14 set in the
/// Kundur save. Other combinations of the same bits are admitted by the bit
/// logic and guarded by the structural anchors in [`read_branch_head`].
fn known_branch_flags(flags: u16) -> bool {
    flags & !0x44B3 == 0x004C
}

/// Locate and walk the branch table after `from`: the first `(count, glue)`
/// candidate whose walk succeeds and after which no further branch record
/// follows (a forged count word inside the glue can parse a prefix of the
/// real table; the true count lands where no further record follows).
fn find_branch_table(
    b: &[u8],
    from: usize,
    bus_ids: &BusIdSet,
    bus_names: &HashMap<String, BusId>,
    runs: &RefCell<HashMap<usize, Run<(Branch, u16)>>>,
    count_can_include_trailer: bool,
) -> Option<Vec<Branch>> {
    // The gap between the shunt table end and the branch count word can
    // exceed one resync window; two cover every observed file.
    let limit = from
        .saturating_add(RESYNC_WINDOW * 2)
        .min(b.len().saturating_sub(4));
    for at in from..limit {
        let count = u32::from_le_bytes(b[at..at + 4].try_into().unwrap()) as usize;
        if count == 0 || count > 10_000_000 {
            continue;
        }
        // The branch table glue is longer than the device tables'; scan a
        // window after the count for the first record.
        let Some(first) = (at.saturating_add(4)..at.saturating_add(64).min(b.len()))
            .find(|&p| read_branch_head(b, p, bus_ids, bus_names).is_ok())
        else {
            continue;
        };
        let counts = [
            Some(count),
            (count_can_include_trailer && count > 1).then_some(count - 1),
        ];
        for effective_count in counts.into_iter().flatten() {
            if !count_fits(b, at.saturating_add(4), effective_count, 24) {
                continue;
            }
            if let Some((branches, after)) =
                branch_run(b, runs, first, effective_count, bus_ids, bus_names)
            {
                // The end check must step exactly like the run: a bit 4 tail on
                // the last record can hold more than one window of blob, and a
                // forged short count ending on such a record would otherwise
                // read as "no further record" and win.
                let last_bit4 = branches.last().is_some_and(|(_, flags)| flags & 0x10 != 0);
                let continues = (after..resync_end(b, after, last_bit4))
                    .any(|p| read_branch_head(b, p, bus_ids, bus_names).is_ok());
                if !continues {
                    return Some(branches.into_iter().map(|(br, _)| br).collect());
                }
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
    bus_names: &HashMap<String, BusId>,
) -> Option<(Vec<(Branch, u16)>, usize)> {
    let mut map = runs.borrow_mut();
    let run = match map.entry(first) {
        Entry::Occupied(e) => e.into_mut(),
        // A failed head parse is not cached, as in the sibling run lookups.
        Entry::Vacant(e) => {
            let (br, end, flags) = read_branch_head(b, first, bus_ids, bus_names).ok()?;
            e.insert(Run::start((br, flags), end))
        }
    };
    run.prefix(count, |after, prev| {
        // The undecoded record tail separates this record from the next;
        // find the next head by bounded scan (see resync_end).
        (after..resync_end(b, after, prev.1 & 0x10 != 0)).find_map(|p| {
            read_branch_head(b, p, bus_ids, bus_names)
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
fn read_branch_head(
    b: &[u8],
    at: usize,
    bus_ids: &BusIdSet,
    bus_names: &HashMap<String, BusId>,
) -> Probe<(Branch, usize, u16)> {
    read_step_up_transformer_head(b, at, bus_ids, bus_names)
        .or_else(|_| read_standard_branch_head(b, at, bus_ids))
}

#[allow(clippy::many_single_char_names)] // r, x, b are the domain names
fn read_standard_branch_head(
    b: &[u8],
    at: usize,
    bus_ids: &BusIdSet,
) -> Probe<(Branch, usize, u16)> {
    let mut c = Cur { b, pos: at };
    let from = branch_endpoint(&mut c)?;
    let to = branch_endpoint(&mut c)?;
    if !bus_ids.contains(from) || !bus_ids.contains(to) || from == to {
        return Err("branch references unknown buses");
    }
    let flags = c.u16()?;
    if !known_branch_flags(flags) {
        return Err("branch record flags not in the validated set");
    }
    let circuit = if flags & 1 == 0 {
        Some(c.short_string_2()?)
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
    let tail_start = c.pos;
    let tag = c.u32()?;
    let (device, tap) = match tag {
        12 => read_modern_branch_tail(&mut c)?,
        5 => read_legacy_branch_tail(&mut c, tail_start)?,
        _ => return Err("branch tail tag not in the validated set"),
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

/// Read a signed branch endpoint. Some saves store a negative endpoint for
/// orientation metadata; the network bus id is the positive magnitude.
fn branch_endpoint(c: &mut Cur<'_>) -> Probe<usize> {
    let raw = i32::from_le_bytes(c.take(4)?.try_into().unwrap());
    raw.checked_abs()
        .and_then(|id| (id > 0).then_some(id as usize))
        .ok_or("invalid branch endpoint")
}

/// Probe the fixed-layout generator step-up transformer records found in the
/// Australian cases. They do not use the normal branch head, so this reader
/// keeps them behind a separate probe with several anchors: known high side
/// bus, 100 MVA nominal marker, plausible device X/MBASE fields, "STEP UP" in
/// the name, and a low side bus named `GEN <unit>`.
fn read_step_up_transformer_head(
    b: &[u8],
    at: usize,
    bus_ids: &BusIdSet,
    bus_names: &HashMap<String, BusId>,
) -> Probe<(Branch, usize, u16)> {
    if at < 8 {
        return Err("step up transformer anchor before record");
    }
    let from = u32_at(b, at)? as usize;
    if !bus_ids.contains(from) {
        return Err("step up transformer high side bus is unknown");
    }
    let nominal = f32_at(b, at - 8)?;
    if !nominal.is_finite() || (nominal - 100.0).abs() > 1e-3 {
        return Err("step up transformer nominal anchor missing");
    }
    let x_device = f32_at(b, checked_offset(at, 17)?)?;
    let mbase = f32_at(b, checked_offset(at, 197)?)?;
    if !x_device.is_finite()
        || !(0.0..=100.0).contains(&x_device)
        || !mbase.is_finite()
        || !(0.1..=1.0e5).contains(&mbase)
    {
        return Err("step up transformer impedance anchor missing");
    }
    let name_at = checked_offset(at, 356)?;
    let name = string_at(b, name_at, 64)?;
    let Some(gen_name) = name.split_whitespace().next() else {
        return Err("step up transformer name is empty");
    };
    if !name.to_ascii_uppercase().contains("STEP UP") {
        return Err("step up transformer name anchor missing");
    }
    let to = bus_names
        .get(&format!("GEN {}", gen_name.to_ascii_uppercase()))
        .copied()
        .ok_or("step up transformer low side bus is unknown")?;
    if to.0 == from {
        return Err("step up transformer has identical endpoints");
    }
    let mut extras = Extras::new();
    extras.insert(LINE_CIRCUIT.into(), serde_json::Value::String(" 1".into()));
    extras.insert(
        BRANCH_DEVICE_TYPE.into(),
        serde_json::Value::String("Transformer".into()),
    );
    let br = Branch {
        from: BusId(from),
        to,
        r: 0.0,
        x: x_device * mbase / MVA_BASE,
        b: 0.0,
        rate_a: 0.0,
        rate_b: 0.0,
        rate_c: 0.0,
        tap: 1.0,
        shift: 0.0,
        in_service: true,
        angmin: -360.0,
        angmax: 360.0,
        extras,
    };
    Ok((
        br,
        checked_offset(checked_offset(name_at, 4)?, name.len())?,
        0,
    ))
}

/// Read the modern branch tail after the common electrical head. The tail is
/// a rating block, separator byte, and kind marker; kind 1 is a line, kind 0
/// is a transformer followed by the tap.
fn read_modern_branch_tail(c: &mut Cur<'_>) -> Probe<(&'static str, f64)> {
    for _ in 0..11 {
        let v = c.f32()?;
        if !v.is_finite() || v.abs() > 1.0e6 {
            return Err("implausible branch rating block value");
        }
    }
    if c.u8()? != 0 {
        return Err("branch record separator byte not zero");
    }
    match c.u8()? {
        0x01 => Ok(("Line", 0.0)),
        0x00 => {
            let tap = c.f32()?;
            if !tap.is_finite() || !(0.2..=5.0).contains(&tap) {
                return Err("implausible transformer tap");
            }
            Ok(("Transformer", tap))
        }
        _ => Err("branch kind marker not in the validated set"),
    }
}

/// Read the older short branch tail. These saves do not carry the modern kind
/// marker, so transformer detection uses the validated zero marker block plus
/// a plausible non-unit tap at the observed tail offset.
fn read_legacy_branch_tail(c: &mut Cur<'_>, tail_start: usize) -> Probe<(&'static str, f64)> {
    for _ in 0..4 {
        if c.u32()? != 0 {
            return Err("legacy branch tail marker is not zero filled");
        }
    }
    let tap = tail_start
        .checked_add(22)
        .and_then(|at| slice_at(c.b, at, 4))
        .and_then(|s| <[u8; 4]>::try_from(s).ok())
        .map_or(0.0, |raw| f64::from(f32::from_le_bytes(raw)));
    if tap.is_finite() && (0.2..=5.0).contains(&tap) && (tap - 1.0).abs() > 1e-6 {
        Ok(("Transformer", tap))
    } else {
        Ok(("Line", 0.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_network(name: &str) -> Network {
        Network {
            name: name.to_string(),
            base_mva: MVA_BASE,
            buses: Vec::new(),
            loads: Vec::new(),
            shunts: Vec::new(),
            branches: Vec::new(),
            generators: Vec::new(),
            storage: Vec::new(),
            hvdc: Vec::new(),
            source_format: SourceFormat::PowerWorldBinary,
            source: None,
        }
    }

    #[test]
    fn best_chain_prefers_valid_chain_over_higher_scoring_error() {
        let mut best = None;
        keep_best_chain(&mut best, 100, Err(unsupported_vintage("bad candidate")));
        keep_best_chain(&mut best, 1, Ok(empty_network("valid")));

        let (_, net) = best.unwrap();
        assert!(net.is_ok());
    }

    #[test]
    fn alternate_load_record_reads_late_p_and_q() {
        let mut bytes = vec![0u8; 41];
        bytes[6] = 0;
        bytes[25..29].copy_from_slice(&1.0f32.to_le_bytes());
        bytes[29..33].copy_from_slice(&0.0f32.to_le_bytes());
        bytes[33..37].copy_from_slice(&0.5f32.to_le_bytes());
        bytes[37..41].copy_from_slice(&0.25f32.to_le_bytes());

        let mut c = Cur { b: &bytes, pos: 6 };
        let load = read_load(&mut c, BusId(1), b"1").unwrap();

        assert!((load.p - 50.0).abs() < 1e-9);
        assert!((load.q - 25.0).abs() < 1e-9);
        assert_eq!(c.pos, 41);
    }
}
