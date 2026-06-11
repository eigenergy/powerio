//! Read substation coordinates from PowerWorld `.pwd` display files
//! (read only).
//!
//! A `.pwd` is the diagram sibling of a case: drawing records for buses,
//! branches, substations, and field labels. This reader decodes the one
//! subset with a differential oracle, the substation symbols, and leaves
//! every other drawing object undecoded. The evidence (seven files across
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

/// Parse the substation coordinates out of `.pwd` bytes.
///
/// # Errors
/// [`Error::FormatRead`] when the header is not the known display shape,
/// the file has no substation identity table (bus only diagrams), or no
/// unique drawing record group links to the identity rows.
pub fn parse_pwd(bytes: &[u8]) -> Result<Vec<PwdSubstation>> {
    let err = |message: String| Error::FormatRead {
        format: FMT,
        message,
    };
    if bytes.len() < 0x40 || u32_at(bytes, 0) != 50 {
        return Err(err(format!(
            "not a recognized PowerWorld display file (header word {}; the probed saves all \
             carry 50)",
            if bytes.len() >= 4 {
                u32_at(bytes, 0)
            } else {
                0
            },
        )));
    }
    if u16_at(bytes, 4) == 0 || u16_at(bytes, 6) == 0 {
        return Err(err("display header canvas dimensions are zero".into()));
    }
    let stamp = u32_at(bytes, 22);
    if stamp == 0 {
        return Err(err(
            "display header stamp is zero; every validated save carries a nonzero stamp the \
             drawing records repeat"
                .into(),
        ));
    }

    let identity = find_identity_table(bytes)?;

    // Every drawing object record repeats the header stamp at +18 and dual
    // encodes its position (f64 at +22/+30, f32 echo at +2/+6); the scan
    // collects every offset with that shape and groups by the u16 type tag.
    let mut groups: Vec<(u16, Vec<usize>)> = Vec::new();
    for i in 0..bytes.len().saturating_sub(38) {
        if u32_at(bytes, i + 18) != stamp {
            continue;
        }
        let x = f64_at(bytes, i + 22);
        let y = f64_at(bytes, i + 30);
        if !x.is_finite() || !y.is_finite() {
            continue;
        }
        #[allow(clippy::cast_possible_truncation)] // the echo is the f32 rounding by design
        let (rx, ry) = (x as f32, y as f32);
        // Bit equality: the magnitude gate below excludes zero, so the only
        // value the echo can hold is the rounded f64 itself.
        if f32_at(bytes, i + 2).to_bits() != rx.to_bits()
            || f32_at(bytes, i + 6).to_bits() != ry.to_bits()
        {
            continue;
        }
        let magnitude = x.abs().max(y.abs());
        if !(1.0..1.0e7).contains(&magnitude) {
            continue;
        }
        let tag = u16_at(bytes, i);
        match groups.iter_mut().find(|(t, _)| *t == tag) {
            Some((_, v)) => v.push(i),
            None => groups.push((tag, vec![i])),
        }
    }

    // The substation group is the one whose records, in stream order, link
    // every identity row in table order: a marker byte (0x03 or 0x07 by
    // era) followed by the row's u32 number, somewhere in the style tail.
    // Field label decoys carry other markers (0x05 observed) or another
    // order and fail; ambiguity is a loud error, never a pick.
    let matches: Vec<&(u16, Vec<usize>)> = groups
        .iter()
        .filter(|(_, offsets)| {
            offsets.len() == identity.len()
                && offsets
                    .iter()
                    .zip(&identity)
                    .all(|(&i, (number, _))| links_number(bytes, i, *number))
        })
        .collect();
    let (_, offsets) = match matches.as_slice() {
        [one] => *one,
        [] => {
            return Err(err(format!(
                "no drawing record group links the {} substation identity rows; the \
                 DisplaySubstation layout of this save is not the validated one",
                identity.len()
            )));
        }
        several => {
            return Err(err(format!(
                "{} drawing record groups link the substation identity rows; refusing to guess \
                 between them",
                several.len()
            )));
        }
    };

    Ok(offsets
        .iter()
        .zip(identity)
        .map(|(&i, (number, name))| PwdSubstation {
            number,
            name,
            x: f64_at(bytes, i + 22),
            y: f64_at(bytes, i + 30),
        })
        .collect())
}

/// The substation identity table: exactly one valid walk behind a
/// `ff ff ff ff 3d 0f` anchor. Zero (bus only diagrams, pre 2016 shapes)
/// and several are loud errors.
fn find_identity_table(b: &[u8]) -> Result<Vec<(u32, String)>> {
    let mut tables = Vec::new();
    for at in memmem(b, &IDENTITY_TAG) {
        if let Some(rows) = identity_walk(b, at + IDENTITY_TAG.len()) {
            tables.push(rows);
        }
    }
    match tables.len() {
        1 => Ok(tables.pop().unwrap()),
        0 => Err(Error::FormatRead {
            format: FMT,
            message: "no substation identity table (tag 0x0f3d walks clean); bus only diagrams \
                      and unprobed save eras are not decoded (see docs/powerworld.md)"
                .into(),
        }),
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
        if at + 4 <= b.len() && b[at..at + 4] == [0xff; 4] {
            return (!rows.is_empty()).then_some(rows);
        }
        if at + 13 > b.len() {
            return None;
        }
        let number = u32_at(b, at);
        if number == 0 || number > 99_999_999 || u32_at(b, at + 4) != number {
            return None;
        }
        let len = u32_at(b, at + 8) as usize;
        if len == 0 || len >= 64 || at + 12 + len + 1 > b.len() {
            return None;
        }
        let name = &b[at + 12..at + 12 + len];
        if !name.iter().all(|&c| (0x20..0x7f).contains(&c)) || b[at + 12 + len] != 0x02 {
            return None;
        }
        if !seen.insert(number) {
            return None;
        }
        rows.push((number, String::from_utf8_lossy(name).into_owned()));
        at += 12 + len + 1;
    }
}

/// Whether the drawing record at `i` links `number`: a marker byte 0x03 or
/// 0x07 (the substation symbol markers of the two observed eras) directly
/// followed by the number, inside the style tail window. The window is
/// variable because a digit string of 1 to 4 characters precedes the link
/// in some saves.
fn links_number(b: &[u8], i: usize, number: u32) -> bool {
    (40..140).any(|d| {
        i + d + 5 <= b.len()
            && (b[i + d] == 0x03 || b[i + d] == 0x07)
            && u32_at(b, i + d + 1) == number
    })
}

/// Every start of `needle` in `haystack`.
fn memmem<'a>(haystack: &'a [u8], needle: &'a [u8]) -> impl Iterator<Item = usize> + 'a {
    haystack
        .windows(needle.len())
        .enumerate()
        .filter_map(move |(i, w)| (w == needle).then_some(i))
}

fn u16_at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes(b[i..i + 2].try_into().unwrap())
}

fn u32_at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes(b[i..i + 4].try_into().unwrap())
}

fn f32_at(b: &[u8], i: usize) -> f32 {
    f32::from_le_bytes(b[i..i + 4].try_into().unwrap())
}

fn f64_at(b: &[u8], i: usize) -> f64 {
    f64::from_le_bytes(b[i..i + 8].try_into().unwrap())
}
