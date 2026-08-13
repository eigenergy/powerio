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

/// Whether this build reads a document stamped `version`.
///
/// A document loads when it shares this build's lineage: the major version once
/// it reaches 1, and the major and minor pair while the major is 0, which is
/// what cargo and Pkg already mean by a 0.x bump. A version this function
/// cannot parse as semver never loads.
#[must_use]
pub fn supports(version: &str) -> bool {
    let Some((major, minor)) = lineage(version) else {
        return false;
    };
    let (current_major, current_minor) = current_lineage();
    major == current_major && (major != 0 || minor == current_minor)
}

/// The message for a document this build does not read.
///
/// `document` names the artifact, spelled as a caller would recognize it
/// (`.pio.json`, `the DC OPF bundle manifest`). An empty `version` is the
/// document that states none, which every release before 0.9.0 wrote.
#[must_use]
pub fn reject(document: &str, version: &str) -> String {
    let states = if version.is_empty() {
        format!("{document} states no `{VERSION_KEY}`, so it was written before powerio 0.9.0")
    } else {
        format!("{document} states `{VERSION_KEY}` {version}")
    };
    format!(
        "{states}; this build reads {}; regenerate it with powerio {VERSION}",
        lineage_label()
    )
}

/// The lineage this build reads, spelled for a message: `0.9.x` while the major
/// is 0, `major version N` afterwards.
#[must_use]
pub fn lineage_label() -> String {
    match current_lineage() {
        (0, minor) => format!("0.{minor}.x"),
        (major, _) => format!("major version {major}"),
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
    fn reject_names_the_document_and_both_versions() {
        let message = reject(".pio.json", "0.2.1");
        assert!(message.contains(".pio.json"), "{message}");
        assert!(message.contains("0.2.1"), "{message}");
        assert!(message.contains(VERSION), "{message}");
    }
}
