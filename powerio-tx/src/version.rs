//! The version every powerio authored document carries, and the rule a reader
//! applies to it.
//!
//! powerio implements case formats and authors none. A file it writes for a
//! foreign format carries that format's own version, which powerio reproduces
//! and never sets. A document powerio authors instead carries [`VERSION`] under
//! the key `powerio_version`, and [`supports`] decides whether this build reads
//! it.

use crate::VERSION;

/// The key every powerio authored document uses for [`VERSION`].
pub const VERSION_KEY: &str = "powerio_version";

/// The 0.x lineage a 1.x build also reads.
///
/// 0.9.0 shipped the documents 1.0 freezes, on purpose: its formats are what
/// 1.0.0 publishes, and 1.0.0 removes deprecated items rather than changing
/// what is written. A gate that refused `0.9.0` at 1.0.0 would make every
/// consumer regenerate an archive whose bytes did not change.
const FROZEN_LINEAGE: (u64, u64) = (0, 9);

/// Whether this build reads a document stamped `version`.
///
/// A document loads when it shares this build's lineage: the major version once
/// it reaches 1, and the major and minor pair while the major is 0, which is
/// what cargo and Pkg already mean by a 0.x bump. A version this function
/// cannot parse as semver never loads.
///
/// One lineage crosses a major boundary: a 1.x build also reads a `0.9`
/// document, because 0.9.0 shipped the formats 1.0 freezes. Nothing else
/// crosses, and 2.0 reads neither.
#[must_use]
pub fn supports(version: &str) -> bool {
    let Some(document) = lineage(version) else {
        return false;
    };
    reads(current_lineage(), document)
}

/// [`supports`], with the build's own lineage supplied rather than read from
/// [`VERSION`]. Split out so the 1.x behavior is testable before 1.x exists.
fn reads(build: (u64, u64), document: (u64, u64)) -> bool {
    let ((build_major, build_minor), (major, minor)) = (build, document);
    if major == build_major {
        return major != 0 || minor == build_minor;
    }
    build_major == 1 && document == FROZEN_LINEAGE
}

/// The diagnosis for a document this build does not read: the release that
/// wrote it (or that it predates the version field) and the lineage this
/// build reads. The remedy is the consumer's to state — a CLI user
/// regenerates with powerio, a browser user re-saves from the source case —
/// so no instruction rides in the message (#375).
///
/// `document` names the artifact, spelled as a caller would recognize it
/// (`.pio.json`, `the DC OPF bundle manifest`). An empty `version` is the
/// document that states none, which every release before 0.9.0 wrote.
#[must_use]
pub fn reject(document: &str, version: &str) -> String {
    let version = bounded_version(version);
    let states = if version.is_empty() {
        format!("{document} states no `{VERSION_KEY}`, so it was written before powerio 0.9.0")
    } else {
        format!("{document} states `{VERSION_KEY}` {version}")
    };
    format!("{states}; this build reads {}", lineage_label())
}

/// The rejection message is one bounded line whatever the document states:
/// the interpolated version keeps at most this many bytes.
pub const MAX_REJECTED_VERSION_BYTES: usize = 64;

/// A stored version reduced to one bounded line: control bytes (newlines and
/// escapes included) become spaces, and the text is truncated at a character
/// boundary with an ellipsis when shortened. A short ordinary version passes
/// through verbatim.
fn bounded_version(version: &str) -> std::borrow::Cow<'_, str> {
    if !version.chars().any(char::is_control) && version.len() <= MAX_REJECTED_VERSION_BYTES {
        return std::borrow::Cow::Borrowed(version);
    }
    let mut line: String = version
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect();
    if line.len() > MAX_REJECTED_VERSION_BYTES {
        let mut end = MAX_REJECTED_VERSION_BYTES;
        while !line.is_char_boundary(end) {
            end -= 1;
        }
        line.truncate(end);
        line.push('\u{2026}');
    }
    std::borrow::Cow::Owned(line)
}

/// The lineage this build reads, spelled for a message: `0.9.x` while the major
/// is 0, `major version N` afterwards. A 1.x build names the 0.x lineage it also
/// reads, so a caller holding a `0.9.0` document is not told to regenerate it.
#[must_use]
pub fn lineage_label() -> String {
    match current_lineage() {
        (0, minor) => format!("0.{minor}.x"),
        (1, _) => format!(
            "major version 1 and {}.{}.x",
            FROZEN_LINEAGE.0, FROZEN_LINEAGE.1
        ),
        (major, _) => format!("major version {major}"),
    }
}

/// The lineage as a path segment: `0.9` while the major is 0, `1` afterwards.
///
/// Names the directory a served JSON Schema lives under, so the published
/// location moves when and only when a document stops loading.
#[must_use]
pub fn lineage_path() -> String {
    match current_lineage() {
        (0, minor) => format!("0.{minor}"),
        (major, _) => major.to_string(),
    }
}

fn current_lineage() -> (u64, u64) {
    lineage(VERSION).expect("the crate version is valid semver")
}

fn lineage(version: &str) -> Option<(u64, u64)> {
    // Accept a semver core `MAJOR.MINOR.PATCH` with an optional prerelease
    // (`-...`) or build (`+...`) tag, so a forward compatible writer that
    // stamps e.g. `0.9.1-rc.1` is not rejected. Split the build tag off first:
    // `+` cannot appear in a prerelease, but a hyphen is legal inside build
    // metadata (`1.0.0+build-x`), so splitting on `-` first would cut inside
    // the build tag and reject a valid version.
    let (rest, build) = match version.split_once('+') {
        Some((rest, build)) => (rest, Some(build)),
        None => (version, None),
    };
    let (core, pre) = match rest.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (rest, None),
    };
    if pre.is_some_and(|s| !valid_suffix(s)) || build.is_some_and(|s| !valid_suffix(s)) {
        return None;
    }
    let mut parts = core.split('.');
    let major = parts.next()?;
    let minor = parts.next()?;
    let patch = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let major = parse_number(major)?;
    let minor = parse_number(minor)?;
    parse_number(patch)?;
    Some((major, minor))
}

fn parse_number(s: &str) -> Option<u64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) || (s.len() > 1 && s.starts_with('0'))
    {
        return None;
    }
    s.parse().ok()
}

fn valid_suffix(s: &str) -> bool {
    !s.is_empty()
        && s.split('.').all(|part| {
            !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lineage_parses_semver_suffixes() {
        assert_eq!(lineage("1.2.3"), Some((1, 2)));
        assert_eq!(lineage("1.0.0-rc.1"), Some((1, 0)));
        // A hyphen inside build metadata is legal semver; splitting on `-`
        // first used to cut inside the build tag and reject the version.
        assert_eq!(lineage("1.0.0+build-x"), Some((1, 0)));
        assert_eq!(lineage("0.9.0"), Some((0, 9)));
    }

    #[test]
    fn lineage_rejects_what_is_not_semver() {
        for bad in [
            "", "1", "1.2", "1.2.3.4", "01.2.3", "1.x.3", "1.2.3-", "1.2.3+",
        ] {
            assert_eq!(lineage(bad), None, "{bad}");
        }
    }

    #[test]
    fn this_build_reads_its_own_version() {
        assert!(supports(VERSION));
    }

    #[test]
    fn a_zero_x_minor_is_its_own_lineage() {
        // While the major is 0 a minor bump is incompatible, so 0.8 and 0.9
        // do not read each other. Both are read by their own patches.
        let (major, minor) = current_lineage();
        assert_eq!(major, 0, "update this test at 1.0.0");
        assert!(supports(&format!("0.{minor}.0")));
        assert!(supports(&format!("0.{minor}.99")));
        assert!(!supports(&format!("0.{}.0", minor + 1)));
        assert!(!supports(&format!("0.{}.0", minor - 1)));
        assert!(!supports("1.0.0"));
    }

    #[test]
    fn one_x_reads_the_lineage_it_froze() {
        // 0.9.0 ships the documents 1.0 publishes, so a 1.x build reads a 0.9
        // document rather than making every consumer regenerate an archive
        // whose bytes did not change. Without this, the release goal — that
        // 0.9.0's formats are 1.0.0's — is false for every document at rest.
        assert!(reads((1, 0), FROZEN_LINEAGE));
        assert!(reads((1, 7), FROZEN_LINEAGE));
        assert!(reads((1, 0), (1, 4)), "a 1.x build reads any 1.x document");
    }

    #[test]
    fn the_frozen_lineage_is_never_behind_this_build() {
        // FROZEN_LINEAGE is the last 0.x, the one a 1.x build also reads. It is
        // a constant with nothing tying it to the crate version, so a 0.x
        // released past it would be a lineage no 1.x build reads. Whoever cuts
        // that release moves the constant or drops the carve-out.
        let (major, minor) = current_lineage();
        if major == 0 {
            assert!(
                (major, minor) <= FROZEN_LINEAGE,
                "0.{minor} ships past the frozen lineage 0.{}",
                FROZEN_LINEAGE.1
            );
        }
    }

    #[test]
    fn nothing_else_crosses_a_major_boundary() {
        assert!(!reads((1, 0), (0, 8)), "only the frozen lineage crosses");
        assert!(!reads((2, 0), FROZEN_LINEAGE), "2.0 froze nothing");
        assert!(!reads((0, 9), (1, 0)), "0.9 cannot read the future");
        assert!(!reads((0, 9), (0, 8)), "a 0.x minor is its own lineage");
    }

    #[test]
    fn reject_is_one_bounded_line_whatever_the_document_states() {
        // A version as large as an admitted document must not be echoed.
        let huge = "9".repeat(1 << 20);
        let message = reject(".pio.json", &huge);
        assert!(
            message.len() < MAX_REJECTED_VERSION_BYTES + 160,
            "{}",
            message.len()
        );
        // Control bytes never reach the one-line message.
        let hostile = "1.0\n\rfake diagnostic: \x1b[31mred";
        let message = reject(".pio.json", hostile);
        assert!(!message.contains('\n'), "{message}");
        assert!(!message.contains('\r'), "{message}");
        assert!(!message.contains('\x1b'), "{message}");
        // A short ordinary version still appears verbatim with the lineage.
        let message = reject(".pio.json", "0.2.1");
        assert!(message.contains("0.2.1"), "{message}");
        assert!(message.contains(&lineage_label()), "{message}");
    }

    #[test]
    fn reject_states_the_diagnosis_and_no_remedy() {
        let message = reject(".pio.json", "0.2.1");
        assert!(message.contains(".pio.json"), "{message}");
        assert!(message.contains("0.2.1"), "{message}");
        assert!(message.contains(&lineage_label()), "{message}");
        // The remedy is the consumer's to state: a CLI user regenerates, a
        // browser user re-saves from the source case (#375).
        assert!(!message.contains("regenerate"), "{message}");
    }
}
