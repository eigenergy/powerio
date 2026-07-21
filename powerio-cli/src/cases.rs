//! Case file discovery, family inference, and loading, shared by the CLI
//! subcommands and the TUI.

use std::path::{Path, PathBuf};

use anyhow::Context;
use powerio_matrix::format::routing::{Detection, JsonClass, SourceFormat as DetectedFormat};
use powerio_matrix::network::Network;
use powerio_pkg::{MulticonductorToBalancedOptions, lower_multiconductor_to_balanced};

/// Extensions (lowercase) that identify a transmission case file.
pub const TRANSMISSION_EXTENSIONS: &[&str] = &["m", "raw", "aux", "epc", "pwb"];

/// Extensions (lowercase) that identify a distribution case file. `.pwd` is
/// the PowerWorld display sibling with no case data and stays excluded.
pub const DISTRIBUTION_EXTENSIONS: &[&str] = &["dss"];

/// `.json` carries no family signal; the shared JSON shape classifier decides.
const JSON_EXTENSION: &str = "json";

/// Extension list for error and empty-state messages; a unit test keeps it in
/// sync with the constants above.
pub const CASE_EXTENSIONS_LABEL: &str = ".m, .raw, .aux, .epc, .pwb, .json, .dss";

/// Infer the case family from clear extensions or, for `.json`, the shared
/// JSON shape classifier. `Some(true)` is distribution, `Some(false)` is
/// transmission, and `None` means the extension carries no family signal.
pub fn infer_input_family(input: &Path) -> anyhow::Result<Option<bool>> {
    let ext = input
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    let Some(ext) = ext.as_deref() else {
        return Ok(None);
    };
    if TRANSMISSION_EXTENSIONS.contains(&ext) {
        return Ok(Some(false));
    }
    if DISTRIBUTION_EXTENSIONS.contains(&ext) {
        return Ok(Some(true));
    }
    if ext != JSON_EXTENSION {
        return Ok(None);
    }
    let text = std::fs::read_to_string(input)
        .with_context(|| format!("reading JSON format markers from {}", input.display()))?;
    match powerio_matrix::format::routing::classify_json_text(&text) {
        JsonClass::Package => anyhow::bail!(
            "{} is a .pio.json package envelope, not a case file; the `package` \
             subcommand writes envelopes, and the bindings read them \
             (powerio.Package.from_json in Python, read_package in Julia)",
            input.display()
        ),
        JsonClass::Case(Detection::Known(DetectedFormat::Distribution(_))) => Ok(Some(true)),
        JsonClass::Case(Detection::Known(DetectedFormat::Transmission(_))) => Ok(Some(false)),
        JsonClass::Case(Detection::Ambiguous) => anyhow::bail!(
            "ambiguous JSON markers in {}; pass --from to choose a format",
            input.display()
        ),
        JsonClass::Case(Detection::Unknown) => anyhow::bail!(
            "cannot infer JSON format for {}; pass --from to choose a format",
            input.display()
        ),
        JsonClass::Case(Detection::Known(_)) => anyhow::bail!(
            "unrecognized JSON format family in {}; pass --from to choose a format",
            input.display()
        ),
    }
}

pub fn looks_like_distribution_input(input: &Path) -> anyhow::Result<bool> {
    Ok(infer_input_family(input)?.unwrap_or(false))
}

/// Recursively list case files under `root`, sorted by path. Hidden entries
/// are pruned, as is the `exclude` directory subtree (the batch output dir,
/// so a rerun never rediscovers its own exports) unless it is `root` itself.
/// A missing or unreadable `root` yields an empty list.
pub fn discover_cases(root: &Path, exclude: Option<&Path>) -> Vec<PathBuf> {
    let excluded = exclude
        .and_then(|p| p.canonicalize().ok())
        .filter(|p| root.canonicalize().ok().as_ref() != Some(p));
    let mut cases: Vec<PathBuf> = walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| !is_hidden(e) && !is_excluded_dir(e, excluded.as_deref()))
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_type().is_file() && has_case_extension(e.path()))
        .map(walkdir::DirEntry::into_path)
        .collect();
    cases.sort();
    cases
}

fn is_hidden(entry: &walkdir::DirEntry) -> bool {
    entry.depth() > 0
        && entry
            .file_name()
            .to_str()
            .is_some_and(|n| n.starts_with('.'))
}

fn is_excluded_dir(entry: &walkdir::DirEntry, excluded: Option<&Path>) -> bool {
    let Some(excluded) = excluded else {
        return false;
    };
    entry.file_type().is_dir() && entry.path().canonicalize().is_ok_and(|p| p == excluded)
}

fn has_case_extension(path: &Path) -> bool {
    let Some(ext) = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
    else {
        return false;
    };
    let ext = ext.as_str();
    TRANSMISSION_EXTENSIONS.contains(&ext)
        || DISTRIBUTION_EXTENSIONS.contains(&ext)
        || ext == JSON_EXTENSION
}

/// A case loaded to the transmission model, whatever family it came from.
pub struct LoadedCase {
    pub network: Network,
    pub warnings: Vec<String>,
}

/// Load one case file as a transmission [`Network`]. Distribution inputs
/// (`.dss`, BMOPF/PMD `.json`) parse to the multiconductor model and go
/// through the explicit balanced lowering pass, whose approximations and
/// dropped fields surface as warnings. A `.dss` redirect fragment (no voltage
/// source) fails the lowering preflight, so a recursive scan skips it instead
/// of exporting a partial feeder.
pub fn load_network(path: &Path) -> anyhow::Result<LoadedCase> {
    let distribution = infer_input_family(path)?
        .ok_or_else(|| anyhow::anyhow!("cannot infer a case format for {}", path.display()))?;
    if distribution {
        let net = powerio_dist::parse_file(path, None)
            .with_context(|| format!("parse {}", path.display()))?;
        let lowered =
            lower_multiconductor_to_balanced(&net, MulticonductorToBalancedOptions::default())
                .map_err(|e| {
                    let diagnostics = e
                        .diagnostics
                        .iter()
                        .map(|d| d.message.as_str())
                        .collect::<Vec<_>>()
                        .join("; ");
                    anyhow::anyhow!("lower {} to balanced: {diagnostics}", path.display())
                })?;
        let mut warnings = net.warnings;
        warnings.extend(
            (lowered.record.approximations.iter())
                .chain(&lowered.record.dropped_fields)
                .map(|s| format!("lowering: {s}")),
        );
        Ok(LoadedCase {
            network: lowered.network,
            warnings,
        })
    } else {
        let parsed = powerio_matrix::parse_file(path, None)
            .with_context(|| format!("parse {}", path.display()))?;
        Ok(LoadedCase {
            network: parsed.network,
            warnings: parsed.warnings,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "").unwrap();
    }

    #[test]
    fn discovers_recursively_sorted_and_prunes() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for rel in [
            "case1.m",
            "a/case2.json",
            "a/net.aux",
            "a/b/c/deep.raw",
            "a/b/c/w.pwb",
            "a/b/c/sys.epc",
            "a/b/feeder.dss",
            "notes.txt",
            "display.pwd",
            ".hidden.m",
            ".git/skip.m",
            "out/case1_meta.json",
        ] {
            touch(&root.join(rel));
        }
        let found = discover_cases(root, Some(&root.join("out")));
        let expected: Vec<PathBuf> = [
            "a/b/c/deep.raw",
            "a/b/c/sys.epc",
            "a/b/c/w.pwb",
            "a/b/feeder.dss",
            "a/case2.json",
            "a/net.aux",
            "case1.m",
        ]
        .iter()
        .map(|rel| root.join(rel))
        .collect();
        assert_eq!(found, expected);
    }

    #[test]
    fn extension_match_ignores_case() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        for rel in ["CASE.M", "net.RAW", "feeder.DSS", "case.Json", "w.PwB"] {
            touch(&root.join(rel));
        }
        assert_eq!(discover_cases(root, None).len(), 5);
    }

    #[test]
    fn exclude_equal_to_root_is_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        touch(&root.join("case1.m"));
        assert_eq!(discover_cases(root, Some(root)), vec![root.join("case1.m")]);
    }

    #[test]
    fn missing_root_yields_empty() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(discover_cases(&tmp.path().join("nope"), None).is_empty());
    }

    #[test]
    fn label_lists_every_extension() {
        for ext in TRANSMISSION_EXTENSIONS
            .iter()
            .chain(DISTRIBUTION_EXTENSIONS)
            .chain(std::iter::once(&JSON_EXTENSION))
        {
            assert!(
                CASE_EXTENSIONS_LABEL.contains(&format!(".{ext}")),
                "label is missing .{ext}"
            );
        }
    }
}
