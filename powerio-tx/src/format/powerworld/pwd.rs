//! Decode PowerWorld `.pwd` drawing coordinates.
//!
//! Supported records provide substation positions, bus positions, and branch
//! paths. Identity tables associate drawing objects with equipment. Repeated
//! coordinates, header stamps, and equipment references validate each record.
//! Ambiguous identity tables and empty decoded drawings return errors.
//!
//! Coordinates retain the drawing's units and orientation. They are not
//! latitude and longitude. Geographic placement requires a separate source
//! such as a case file or an AUX file with geographic fields.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use crate::{Error, Result};

const FMT: &str = "PowerWorld .pwd";

/// The identity table tag behind the `ff ff ff ff` sentinel.
const IDENTITY_TAG: [u8; 6] = [0xff, 0xff, 0xff, 0xff, 0x3d, 0x0f];

/// The word every probed save writes one u32 past the header stamp. It pins
/// which of the two candidate positions holds the stamp when a canvas title
/// shifts it (see [`parse_pwd_header`]).
const HEADER_TRAILER: u32 = 10105;

/// Where the stamp sits when the header carries no canvas title.
const UNTITLED_STAMP_AT: usize = 22;

/// Longest canvas title the header shift accepts. Every probed save is well
/// under it; a length past it is a corrupt or unrecognized header, not a
/// title, so the reader falls back to the untitled position.
const MAX_TITLE_LEN: usize = 256;

/// Cap on identity record steps across every anchor in one parse. A step is one
/// record examined; each consumes at least 13 bytes, so the largest real table
/// is far below this. Bounds the anchors × records blowup a crafted file could
/// otherwise force. Matches the probe-budget idiom of the `.pwb` reader.
const IDENTITY_WALK_BUDGET: u64 = 128_000_000;

/// Cap on identity rows retained across every candidate walk in one parse.
/// The step budget above bounds the reader's *work*; this bounds what a
/// densely packed file can make it *hold*: each retained row owns a name
/// String, so without a retention cap a GB-scale file of valid-looking
/// records could drive several GB of held rows before the walk finishes
/// (#274). The vendored ACTIVSg200 display retains 200 rows; a display for
/// the largest interconnection-scale case stays in the tens of thousands, so
/// one million rows and 64 MiB of name bytes are far above any real layout
/// while capping amplification near the input's own size. Exceeding either
/// is a coded refusal, never a silent omission.
const IDENTITY_ROW_BUDGET: usize = 1_000_000;
const IDENTITY_NAME_BYTE_BUDGET: usize = 64 << 20;

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
/// A `.pwd` is not a case file and does not carry a [`BalancedNetwork`](crate::BalancedNetwork).
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

/// Decode supported substation, bus, and branch drawing objects into a shared layer.
/// Bus and line object records require matching case identities and repeated positions.
pub fn parse_pwd_layer(bytes: &[u8]) -> Result<crate::geo::GeoParsed> {
    use crate::geo::{ElementKey, GeoFeature, GeoGeometry, GeoTarget};
    let display = parse_pwd_display(bytes)?;
    let mut layer = crate::geo::to_geo_layer_from_pwd(&display);
    let mut diagnostics = Vec::new();
    let identities = bus_identities(bytes)?;
    if !identities.is_empty() {
        let mut buses = BTreeMap::new();
        let mut owners = Vec::new();
        for at in 0..bytes.len().saturating_sub(38) {
            let Some((x, y)) = drawing_position(bytes, at, display.stamp) else {
                continue;
            };
            match u16_at(bytes, at) {
                Some(0x277e) => {
                    let Some(end) = style_label_end(bytes, at, &[67]) else {
                        continue;
                    };
                    let marker = end + 17;
                    if bytes.get(marker) != Some(&3) {
                        continue;
                    }
                    let Some(number) =
                        u32_at(bytes, marker + 1).filter(|n| identities.contains_key(n))
                    else {
                        continue;
                    };
                    if buses.insert(number, [x, y]).is_some() {
                        return Err(pwd_err("duplicate bus drawing identity"));
                    }
                }
                Some(0x27b3 | 0x27b7) => owners.push(at),
                _ => {}
            }
        }
        for (&number, &point) in &buses {
            layer.features.push(GeoFeature {
                target: GeoTarget::Bus,
                key: ElementKey {
                    id: Some(number.to_string()),
                    name: Some(identities[&number].clone()),
                    ..ElementKey::default()
                },
                geometry: GeoGeometry::Point(point),
                from: None,
                to: None,
                kind: None,
            });
        }
        let mut unsupported = identities.len().saturating_sub(buses.len());
        for at in owners {
            let decoded = decode_branch_drawing(bytes, at, display.stamp, &buses);
            if let Some(feature) = decoded {
                layer.features.push(feature);
            } else {
                unsupported += 1;
            }
        }
        if unsupported > 0 {
            diagnostics.push(crate::diagnostics::Diagnostic::of(&crate::diagnostics::codes::READ_GEO_SOURCE_MALFORMED,
                format!("{unsupported} bus or branch drawing objects have unsupported layouts or unmatched identities")));
        }
    }
    if layer.features.is_empty() {
        return Err(pwd_err(
            "no supported bus, branch, or substation positions were decoded from the drawing",
        ));
    }
    Ok(crate::geo::GeoParsed { layer, diagnostics })
}

fn decode_branch_drawing(
    bytes: &[u8],
    at: usize,
    stamp: u32,
    buses: &BTreeMap<u32, [f64; 2]>,
) -> Option<crate::geo::GeoFeature> {
    use crate::geo::{ElementKey, GeoFeature, GeoGeometry, GeoTarget};
    let end = style_label_end(bytes, at, &[75, 79])?;
    let from = u32_at(bytes, end + 63)?;
    let to = u32_at(bytes, end + 67)?;
    if !buses.contains_key(&from) || !buses.contains_key(&to) {
        return None;
    }
    let child = end + 72;
    if u16_at(bytes, child) != Some(0x3131) || drawing_position(bytes, child, stamp).is_none() {
        return None;
    }
    let child_end = style_label_end(bytes, child, &[75, 79])?;
    let count_at = child_end + 58;
    let count = u32_at(bytes, count_at)? as usize;
    if !(2..=100_000).contains(&count) {
        return None;
    }
    let raw = bytes.get(count_at + 4..count_at.checked_add(4 + count * 16)?)?;
    let mut points = Vec::with_capacity(count);
    for i in (0..raw.len()).step_by(16) {
        let x = f64_at(raw, i)?;
        let y = f64_at(raw, i + 8)?;
        if !x.is_finite() || !y.is_finite() || x.abs().max(y.abs()) > 1e7 {
            return None;
        }
        points.push([x, y]);
    }
    Some(GeoFeature {
        target: GeoTarget::Branch,
        key: ElementKey::default(),
        geometry: GeoGeometry::LineString(points),
        from: Some(from.to_string()),
        to: Some(to.to_string()),
        kind: None,
    })
}

fn drawing_position(bytes: &[u8], at: usize, stamp: u32) -> Option<(f64, f64)> {
    if u32_at(bytes, at + 18) != Some(stamp) {
        return None;
    }
    let x = f64_at(bytes, at + 22)?;
    let y = f64_at(bytes, at + 30)?;
    if !x.is_finite() || !y.is_finite() || x.abs().max(y.abs()) > 1e7 {
        return None;
    }
    #[allow(clippy::cast_possible_truncation)]
    let (rx, ry) = (x as f32, y as f32);
    if f32_at(bytes, at + 2)?.to_bits() != rx.to_bits()
        || f32_at(bytes, at + 6)?.to_bits() != ry.to_bits()
    {
        return None;
    }
    Some((x, y))
}

fn style_label_end(bytes: &[u8], at: usize, offsets: &[usize]) -> Option<usize> {
    let mut found = None;
    for offset in offsets {
        let length = u32_at(bytes, at + offset)? as usize;
        if length == 0 || length > 64 {
            continue;
        }
        let end = (at + offset + 4).checked_add(length)?;
        let text = bytes.get(at + offset + 4..end)?;
        if !text.iter().all(|c| (0x20..0x7f).contains(c)) || u32_at(bytes, end) != Some(u32::MAX) {
            continue;
        }
        if found.replace(end).is_some() {
            return None;
        }
    }
    found
}

fn bus_identities(bytes: &[u8]) -> Result<BTreeMap<u32, String>> {
    let mut found = None;
    let mut work = 0usize;
    for anchor in memmem(bytes, &[0x3c, 0x0f]) {
        let mut at = anchor + 2;
        let mut rows = BTreeMap::new();
        while work < IDENTITY_ROW_BUDGET {
            work += 1;
            if bytes.get(at..at + 6) == Some(&IDENTITY_TAG) {
                if !rows.is_empty() && found.replace(rows).is_some() {
                    return Err(pwd_err("ambiguous bus identity tables"));
                }
                break;
            }
            let row = (|| -> Option<(u32, String, usize)> {
                let number = u32_at(bytes, at)?;
                let length = u32_at(bytes, at + 4)? as usize;
                if number == 0 || number > 99_999_999 || length == 0 || length > 64 {
                    return None;
                }
                let end = at + 8 + length;
                let name = bytes.get(at + 8..end)?;
                if !name.iter().all(|c| (0x20..0x7f).contains(c))
                    || u32_at(bytes, end) != Some(number)
                    || bytes.get(end + 4) != Some(&0)
                {
                    return None;
                }
                let label_length = u32_at(bytes, end + 5)? as usize;
                if label_length > 64 {
                    return None;
                }
                let kv = f32_at(bytes, end + 9 + label_length)?;
                if !kv.is_finite() || !(0.0..=10_000.0).contains(&kv) {
                    return None;
                }
                Some((
                    number,
                    String::from_utf8_lossy(name).into_owned(),
                    end + 13 + label_length,
                ))
            })();
            let Some((number, name, next)) = row else {
                break;
            };
            if rows.insert(number, name).is_some() {
                break;
            }
            at = next;
        }
        if work >= IDENTITY_ROW_BUDGET {
            return Err(pwd_err("bus identity search exceeded its work limit"));
        }
    }
    Ok(found.unwrap_or_default())
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
    let stamp = header_stamp(bytes).unwrap_or(0);
    if stamp == 0 {
        return Err(pwd_err(
            "display header stamp is zero; every validated save carries a nonzero stamp the \
             drawing records repeat",
        ));
    }
    Ok((canvas_width, canvas_height, stamp))
}

/// The per file stamp every drawing object record repeats at +18.
///
/// A save with no canvas title puts it at offset 22. A save with one writes a
/// u16 length at offset 10, a zero u16, the title text, and eight zero bytes,
/// which shifts the stamp by the title length, so offset 22 holds title text
/// and reads as a zero stamp. The shifted position is taken only when the
/// whole title structure validates and the word past the stamp is the trailer
/// every probed save writes, so a header that is not this shape reads the
/// untitled position.
fn header_stamp(bytes: &[u8]) -> Option<u32> {
    let title_len = usize::from(u16_at(bytes, 10)?);
    let titled_at = UNTITLED_STAMP_AT.checked_add(title_len)?;
    let title_is_shaped = title_len <= MAX_TITLE_LEN
        && u16_at(bytes, 12) == Some(0)
        && bytes
            .get(14..14 + title_len)
            .is_some_and(|title| title.iter().all(|&c| (0x20..0x7f).contains(&c)))
        && bytes
            .get(14 + title_len..UNTITLED_STAMP_AT + title_len)
            .is_some_and(|gap| gap.iter().all(|&c| c == 0));
    if title_is_shaped
        && u32_at(bytes, titled_at).is_some_and(|stamp| stamp != 0)
        && u32_at(bytes, titled_at + 4) == Some(HEADER_TRAILER)
    {
        return u32_at(bytes, titled_at);
    }
    u32_at(bytes, UNTITLED_STAMP_AT)
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
    // Keyed by type tag so grouping is O(log tags) per record: a crafted file
    // can spread gate-passing records across up to 65536 distinct tags, and a
    // linear scan per record would be quadratic in the file size.
    let mut groups: BTreeMap<u16, Vec<DrawRecord>> = BTreeMap::new();
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
        groups.entry(tag).or_default().push(rec);
    }

    // The substation group is the one whose records, in stream order, link
    // every identity row in table order: a marker byte (0x03 or 0x07 by
    // era) followed by the row's u32 number, somewhere in the style tail.
    // Field label decoys carry other markers (0x05 observed) or another
    // order and fail; ambiguity is a loud error, never a pick.
    let matches: Vec<(&u16, &Vec<DrawRecord>)> = groups
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
    // A crafted file can plant many IDENTITY_TAG anchors, each starting a walk
    // that runs to a sentinel, so the total work is anchors × records. One
    // shared budget over every record step across every anchor keeps that
    // bounded; the largest real identity table is orders of magnitude below it.
    let mut budget = 0u64;
    let mut retained = Retention::default();
    let mut tables = Vec::new();
    for at in memmem(b, &IDENTITY_TAG) {
        if let Some(rows) = identity_walk(b, at + IDENTITY_TAG.len(), &mut budget, &mut retained) {
            tables.push(rows);
        }
        if budget > IDENTITY_WALK_BUDGET {
            return Err(Error::FormatRead {
                format: FMT,
                message: "substation identity search exceeded its probe budget; the file is \
                          not a decodable DisplaySubstation layout"
                    .into(),
            });
        }
        if retained.exceeded {
            return Err(Error::FormatRead {
                format: FMT,
                message: "substation identity search exceeded its retention budget; the file \
                          packs more identity rows than any decodable DisplaySubstation \
                          layout states"
                    .into(),
            });
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
/// Rows and name bytes retained across every walk of one parse, with the
/// flag that turns exhaustion into the coded refusal rather than a silently
/// shorter table (#274).
#[derive(Default)]
struct Retention {
    rows: usize,
    name_bytes: usize,
    exceeded: bool,
}

fn identity_walk(
    b: &[u8],
    mut at: usize,
    budget: &mut u64,
    retained: &mut Retention,
) -> Option<Vec<(u32, String)>> {
    let mut rows = Vec::new();
    let mut seen = HashSet::new();
    loop {
        // One record step; abandon the walk once the shared budget is spent so
        // a file packed with anchors cannot force quadratic work.
        *budget = budget.saturating_add(1);
        if *budget > IDENTITY_WALK_BUDGET {
            return None;
        }
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
        retained.rows += 1;
        retained.name_bytes += name.len();
        if retained.rows > IDENTITY_ROW_BUDGET || retained.name_bytes > IDENTITY_NAME_BYTE_BUDGET {
            retained.exceeded = true;
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

#[cfg(test)]
mod retention_tests {
    use super::*;

    /// #274: the retention budget refuses a file densely packed with valid
    /// identity rows, with a coded error rather than a silently shorter
    /// table or GB-scale held rows.
    #[test]
    fn packed_identity_rows_hit_the_retention_budget() {
        // Header word 50, nonzero canvas, then one anchor followed by more
        // valid-looking rows than any decodable layout states.
        let mut b = vec![0u8; 0x40];
        b[0] = 50;
        b[4] = 1; // canvas width
        b[6] = 1; // canvas height
        b[22] = 7; // nonzero stamp
        b.extend_from_slice(&IDENTITY_TAG);
        let name = b"S";
        for number in 1..=(IDENTITY_ROW_BUDGET as u32 + 2) {
            b.extend_from_slice(&number.to_le_bytes());
            b.extend_from_slice(&number.to_le_bytes());
            b.extend_from_slice(&(name.len() as u32).to_le_bytes());
            b.extend_from_slice(name);
            b.push(0x02);
        }
        b.extend_from_slice(&[0xff; 4]);
        let error = parse_pwd(&b).unwrap_err().to_string();
        assert!(error.contains("retention budget"), "{error}");
    }
}

#[cfg(test)]
mod drawing_tests {
    use super::*;

    fn symbol(tag: u16, stamp: u32, x: f64, y: f64, size: usize) -> Vec<u8> {
        let mut b = vec![0; size];
        b[..2].copy_from_slice(&tag.to_le_bytes());
        b[2..6].copy_from_slice(&(x as f32).to_le_bytes());
        b[6..10].copy_from_slice(&(y as f32).to_le_bytes());
        b[18..22].copy_from_slice(&stamp.to_le_bytes());
        b[22..30].copy_from_slice(&x.to_le_bytes());
        b[30..38].copy_from_slice(&y.to_le_bytes());
        b
    }

    fn drawing() -> Vec<u8> {
        let mut b = vec![0; 64];
        b[..4].copy_from_slice(&50u32.to_le_bytes());
        b[4..6].copy_from_slice(&200u16.to_le_bytes());
        b[6..8].copy_from_slice(&200u16.to_le_bytes());
        b[22..26].copy_from_slice(&7u32.to_le_bytes());
        b[26..30].copy_from_slice(&10105u32.to_le_bytes());
        b.extend_from_slice(&[0x3c, 0x0f]);
        for number in [1u32, 2] {
            b.extend_from_slice(&number.to_le_bytes());
            b.extend_from_slice(&1u32.to_le_bytes());
            b.push(b'A');
            b.extend_from_slice(&number.to_le_bytes());
            b.push(0);
            b.extend_from_slice(&0u32.to_le_bytes());
            b.extend_from_slice(&345f32.to_le_bytes());
        }
        b.extend_from_slice(&IDENTITY_TAG);
        b.extend_from_slice(&[0xff, 0xff, 0xff, 0xff, 0x3e, 0x0f]);
        for (number, x, y) in [(1u32, 28500., 18900.), (2u32, 29500., 17900.)] {
            let mut row = symbol(0x277e, 7, x, y, 96);
            row[67..71].copy_from_slice(&1u32.to_le_bytes());
            row[71] = b'A';
            row[72..76].fill(0xff);
            row[89] = 3;
            row[90..94].copy_from_slice(&number.to_le_bytes());
            b.extend(row);
        }
        let mut line = symbol(0x27b3, 7, 29000., 18400., 156);
        line[79..83].copy_from_slice(&1u32.to_le_bytes());
        line[83] = b'A';
        line[84..88].fill(0xff);
        line[147..151].copy_from_slice(&1u32.to_le_bytes());
        line[151..155].copy_from_slice(&2u32.to_le_bytes());
        b.extend(line);
        let mut route = symbol(0x3131, 7, 29000., 18400., 178);
        route[79..83].copy_from_slice(&1u32.to_le_bytes());
        route[83] = b'A';
        route[84..88].fill(0xff);
        route[142..146].copy_from_slice(&2u32.to_le_bytes());
        for (i, value) in [28500f64, 18900., 29500., 17900.].iter().enumerate() {
            route[146 + i * 8..154 + i * 8].copy_from_slice(&value.to_le_bytes());
        }
        b.extend(route);
        b
    }

    #[test]
    fn bus_and_route_positions_keep_diagram_units() {
        let bytes = drawing();
        let parsed = parse_pwd_layer(&bytes).unwrap();
        assert_eq!(parsed.diagnostics.as_slice(), []);
        assert_eq!(parsed.layer.features.len(), 3);
        assert!(matches!(
            parsed.layer.space,
            crate::geo::CoordinateSpace::Diagram { .. }
        ));
        assert_eq!(
            parsed.layer.features[0].geometry,
            crate::geo::GeoGeometry::Point([28500., 18900.])
        );
        let route = &parsed.layer.features[2];
        assert_eq!(route.from.as_deref(), Some("1"));
        assert_eq!(route.to.as_deref(), Some("2"));
        let encoded = parsed.layer.to_geojson_checked().unwrap();
        assert_eq!(
            crate::geo::GeoLayer::parse(&encoded, None).unwrap().layer,
            parsed.layer
        );
        let truncated = parse_pwd_layer(&bytes[..bytes.len() - 8]).unwrap();
        assert_ne!(truncated.diagnostics.as_slice(), []);
        assert_eq!(truncated.layer.features.len(), 2);
    }

    #[test]
    fn empty_and_mismatched_drawing_records_fail_explicitly() {
        let mut b = drawing();
        assert!(parse_pwd_layer(&b[..64]).is_err());
        let starts: Vec<_> = memmem(&b, &[0x7e, 0x27]).collect();
        for at in starts {
            b[at + 2..at + 6].fill(0);
        }
        assert!(parse_pwd_layer(&b).is_err());
    }
}
