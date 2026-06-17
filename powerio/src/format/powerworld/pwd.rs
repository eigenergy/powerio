//! Read substation coordinates from PowerWorld `.pwd` display files
//! (read only).
//!
//! A `.pwd` is the diagram sibling of a case: drawing records for buses,
//! branches, substations, and field labels. This reader decodes the one
//! subset with a differential oracle, the substation symbols, and leaves
//! every other drawing object undecoded. Files without the substation table
//! still return display metadata with an empty substation list. The evidence
//! (seven files across
//! the 2016 through 2022 writer eras, each matched 1-1 against the
//! latitude/longitude its same vintage aux carries per substation, except
//! the v19 resave, which matches 1248/1250 against the published case
//! across a vintage skew) is in `docs/powerworld.md`.
//!
//! Two structures carry the data, both present in every probed save:
//!
//! - The substation identity table, behind the only `ff ff ff ff 3d 0f`
//!   byte sequence in the file (sentinel plus table tag 0x0f3d): records of
//!   `u32 number, u32 number (exact duplicate), u32 length, name, 0x02`,
//!   terminated exactly by the next `ff ff ff ff`. The order is display
//!   order, not case order.
//! - The DisplaySubstation drawing records: each repeats the file's header
//!   stamp (the u32 at offset 22) at +18, stores the position as f64 x/y at
//!   +22/+30 with an f32 echo of both at +2/+6, and links its substation
//!   number behind a marker byte (0x03 or 0x07 by writer era) in the style
//!   tail. The record's type tag (the u16 at +0) varies per save, so the
//!   reader keys on this structure instead: stamp echo, dual encoded
//!   coordinates, and a link to every identity row in table order. Decoy
//!   groups exist (field label records with the same count and plausible
//!   coordinates) and fail the link gauntlet; if more than one group ever
//!   passes, the reader rejects rather than guesses.
//!
//! The coordinates are diagram positions (y north positive), not
//! geography: no probed file stores latitude or longitude directly. The
//! auto generated TAMU and Hawaii layouts equal a Mercator projection
//! (`x = k * longitude`, `y = k * merc(latitude)`, k = 535.81608... on the
//! never edited Hawaii40 file, bit exact), but hand moved symbols and the
//! June 2016 era deviate, so the values are exposed as stored and any
//! projection is the consumer's choice. Consumers wanting geography should
//! read the aux.

use std::collections::HashSet;
use std::path::Path;

use crate::{Error, Result};

const FMT: &str = "PowerWorld .pwd";

/// The identity table tag behind the `ff ff ff ff` sentinel.
const IDENTITY_TAG: [u8; 6] = [0xff, 0xff, 0xff, 0xff, 0x3d, 0x0f];

/// One substation symbol from a display file: the identity row joined with
/// its drawing record, in identity table (display) order. `x` and `y` are
/// diagram coordinates as stored, y north positive (see the module docs).
#[derive(Debug, Clone, PartialEq)]
pub struct PwdSubstation {
    pub number: u32,
    pub name: String,
    pub x: f64,
    pub y: f64,
}

/// Decoded PowerWorld display file content.
///
/// A `.pwd` is not a case file and does not carry a [`Network`](crate::Network).
/// This structure exposes the display metadata the reader validates plus the
/// supported drawing object subset.
#[derive(Debug, Clone, PartialEq)]
pub struct PwdDisplay {
    pub canvas_width: u16,
    pub canvas_height: u16,
    pub stamp: u32,
    pub substations: Vec<PwdSubstation>,
}

/// Read and parse a `.pwd` display file.
///
/// # Errors
/// [`Error::Io`] when the file cannot be read, or [`Error::FormatRead`] when
/// the display bytes are not a supported PowerWorld `.pwd` shape.
pub fn parse_pwd_file(path: impl AsRef<Path>) -> Result<PwdDisplay> {
    let bytes = std::fs::read(path)?;
    parse_pwd_display(&bytes)
}

/// Parse a `.pwd` display file, returning metadata and decoded substations.
///
/// # Errors
/// [`Error::FormatRead`] when the header is not the known display shape,
/// or no unique drawing record group links to the identity rows.
pub fn parse_pwd_display(bytes: &[u8]) -> Result<PwdDisplay> {
    parse_pwd_inner(bytes)
}

/// Parse the substation coordinates out of `.pwd` bytes.
///
/// # Errors
/// [`Error::FormatRead`] when the header is not the known display shape,
/// or no unique drawing record group links to the identity rows.
pub fn parse_pwd(bytes: &[u8]) -> Result<Vec<PwdSubstation>> {
    parse_pwd_display(bytes).map(|display| display.substations)
}

fn pwd_err(message: impl Into<String>) -> Error {
    Error::FormatRead {
        format: FMT,
        message: message.into(),
    }
}

fn parse_pwd_header(bytes: &[u8]) -> Result<(u16, u16, u32)> {
    let (Some(header), Some(canvas_width), Some(canvas_height)) =
        (u32_at(bytes, 0), u16_at(bytes, 4), u16_at(bytes, 6))
    else {
        let header = u32_at(bytes, 0).unwrap_or(0);
        return Err(pwd_err(format!(
            "not a recognized PowerWorld display file (header word {header}; the probed saves all \
             carry 50)",
        )));
    };
    if bytes.len() < 0x40 || header != 50 {
        return Err(pwd_err(format!(
            "not a recognized PowerWorld display file (header word {header}; the probed saves all \
             carry 50)",
        )));
    }
    if canvas_width == 0 || canvas_height == 0 {
        return Err(pwd_err("display header canvas dimensions are zero"));
    }
    let stamp = u32_at(bytes, 22).unwrap_or(0);
    if stamp == 0 {
        return Err(pwd_err(
            "display header stamp is zero; every validated save carries a nonzero stamp the \
             drawing records repeat",
        ));
    }
    Ok((canvas_width, canvas_height, stamp))
}

fn parse_pwd_inner(bytes: &[u8]) -> Result<PwdDisplay> {
    let (canvas_width, canvas_height, stamp) = parse_pwd_header(bytes)?;

    let identity = find_identity_table(bytes)?;
    if identity.is_empty() {
        return Ok(PwdDisplay {
            canvas_width,
            canvas_height,
            stamp,
            substations: Vec::new(),
        });
    }

    // Every drawing object record repeats the header stamp at +18 and dual
    // encodes its position (f64 at +22/+30, f32 echo at +2/+6); the scan
    // collects every offset with that shape and groups by the u16 type tag.
    let mut groups: Vec<(u16, Vec<DrawRecord>)> = Vec::new();
    for i in 0..bytes.len().saturating_sub(38) {
        if u32_at(bytes, i + 18) != Some(stamp) {
            continue;
        }
        let (Some(x), Some(y)) = (f64_at(bytes, i + 22), f64_at(bytes, i + 30)) else {
            continue;
        };
        if !x.is_finite() || !y.is_finite() {
            continue;
        }
        #[allow(clippy::cast_possible_truncation)] // the echo is the f32 rounding by design
        let (rx, ry) = (x as f32, y as f32);
        // Bit equality: the magnitude gate below excludes zero, so the only
        // value the echo can hold is the rounded f64 itself.
        if f32_at(bytes, i + 2).map(f32::to_bits) != Some(rx.to_bits())
            || f32_at(bytes, i + 6).map(f32::to_bits) != Some(ry.to_bits())
        {
            continue;
        }
        let magnitude = x.abs().max(y.abs());
        if !(1.0..1.0e7).contains(&magnitude) {
            continue;
        }
        let Some(tag) = u16_at(bytes, i) else {
            continue;
        };
        let rec = DrawRecord { at: i, x, y };
        match groups.iter_mut().find(|(t, _)| *t == tag) {
            Some((_, v)) => v.push(rec),
            None => groups.push((tag, vec![rec])),
        }
    }

    // The substation group is the one whose records, in stream order, link
    // every identity row in table order: a marker byte (0x03 or 0x07 by
    // era) followed by the row's u32 number, somewhere in the style tail.
    // Field label decoys carry other markers (0x05 observed) or another
    // order and fail; ambiguity is a loud error, never a pick.
    let matches: Vec<&(u16, Vec<DrawRecord>)> = groups
        .iter()
        .filter(|(_, records)| {
            records.len() == identity.len()
                && records
                    .iter()
                    .zip(&identity)
                    .all(|(rec, (number, _))| links_number(bytes, rec.at, *number))
        })
        .collect();
    let (_, records) = match matches.as_slice() {
        [one] => *one,
        [] => {
            return Err(pwd_err(format!(
                "no drawing record group links the {} substation identity rows; the \
                 DisplaySubstation layout of this save is not the validated one",
                identity.len()
            )));
        }
        several => {
            return Err(pwd_err(format!(
                "{} drawing record groups link the substation identity rows; refusing to guess \
                 between them",
                several.len()
            )));
        }
    };

    let substations = records
        .iter()
        .zip(identity)
        .map(|(rec, (number, name))| PwdSubstation {
            number,
            name,
            x: rec.x,
            y: rec.y,
        })
        .collect();
    Ok(PwdDisplay {
        canvas_width,
        canvas_height,
        stamp,
        substations,
    })
}

/// A drawing record that passed the shape gate: its stream offset (for the
/// identity link check) and the decoded coordinates, kept so the final mapping
/// never re-reads the bytes.
struct DrawRecord {
    at: usize,
    x: f64,
    y: f64,
}

/// The substation identity table: exactly one valid walk behind a
/// `ff ff ff ff 3d 0f` anchor. A missing table means there are no decoded
/// substation symbols. Several tables are a loud error.
fn find_identity_table(b: &[u8]) -> Result<Vec<(u32, String)>> {
    let mut tables = Vec::new();
    for at in memmem(b, &IDENTITY_TAG) {
        if let Some(rows) = identity_walk(b, at + IDENTITY_TAG.len()) {
            tables.push(rows);
        }
    }
    match tables.len() {
        1 => Ok(tables.pop().unwrap()),
        0 => Ok(Vec::new()),
        n => Err(Error::FormatRead {
            format: FMT,
            message: format!(
                "{n} byte ranges walk as a substation identity table; refusing to guess \
                 between them"
            ),
        }),
    }
}

/// Walk identity records (`u32 number, u32 duplicate, u32 length, name,
/// 0x02`) from `at` until the next `ff ff ff ff` sentinel, which must
/// arrive exactly at a record boundary. At least one record, numbers
/// unique and plausible, names printable.
fn identity_walk(b: &[u8], mut at: usize) -> Option<Vec<(u32, String)>> {
    let mut rows = Vec::new();
    let mut seen = HashSet::new();
    loop {
        if b.get(at..).and_then(|s| s.get(..4)) == Some([0xff; 4].as_slice()) {
            return (!rows.is_empty()).then_some(rows);
        }
        let number = u32_at(b, at)?;
        let duplicate_at = at.checked_add(4)?;
        if number == 0 || number > 99_999_999 || u32_at(b, duplicate_at) != Some(number) {
            return None;
        }
        let len_at = at.checked_add(8)?;
        let len = u32_at(b, len_at)? as usize;
        if len == 0 || len >= 64 {
            return None;
        }
        let name_start = at.checked_add(12)?;
        let name_end = name_start.checked_add(len)?;
        let name = b.get(name_start..name_end)?;
        if !name.iter().all(|&c| (0x20..0x7f).contains(&c)) || b.get(name_end) != Some(&0x02) {
            return None;
        }
        if !seen.insert(number) {
            return None;
        }
        rows.push((number, String::from_utf8_lossy(name).into_owned()));
        at = name_end.checked_add(1)?;
    }
}

/// Whether the drawing record at `i` links `number`: a marker byte 0x03 or
/// 0x07 (the substation symbol markers of the two observed eras) directly
/// followed by the number, inside the style tail window. The window is
/// variable because a digit string of 1 to 4 characters precedes the link
/// in some saves.
fn links_number(b: &[u8], i: usize, number: u32) -> bool {
    (40..140).any(|d| {
        let Some(marker_at) = i.checked_add(d) else {
            return false;
        };
        let Some(number_at) = marker_at.checked_add(1) else {
            return false;
        };
        matches!(b.get(marker_at), Some(0x03 | 0x07)) && u32_at(b, number_at) == Some(number)
    })
}

/// Every start of `needle` in `haystack`.
fn memmem<'a>(haystack: &'a [u8], needle: &'a [u8]) -> impl Iterator<Item = usize> + 'a {
    haystack
        .windows(needle.len())
        .enumerate()
        .filter_map(move |(i, w)| (w == needle).then_some(i))
}

// Total little endian reads: `None` past the end of the buffer, no index
// arithmetic that can panic or wrap. Every offset in this reader derives
// from untrusted file bytes, so the accessors carry the bounds check.

fn u16_at(b: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_le_bytes(*b.get(i..)?.first_chunk()?))
}

fn u32_at(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(*b.get(i..)?.first_chunk()?))
}

fn f32_at(b: &[u8], i: usize) -> Option<f32> {
    Some(f32::from_le_bytes(*b.get(i..)?.first_chunk()?))
}

fn f64_at(b: &[u8], i: usize) -> Option<f64> {
    Some(f64::from_le_bytes(*b.get(i..)?.first_chunk()?))
}
