//! Case file discovery, family inference, and loading, shared by the CLI
//! subcommands and the TUI.

use std::path::{Path, PathBuf};

use anyhow::Context;
use powerio::BalancedNetwork;
use powerio::transform::{MulticonductorToBalancedOptions, to_balanced_network};
use powerio_tx::format::routing::{Detection, JsonClass, SourceFormat as DetectedFormat};

/// Extensions (lowercase) that identify a transmission case file. `.cdf`
/// names an IEEE CDF case; the archives' `.txt` spelling is not discovered
/// because a text file of any kind carries that extension.
pub const TRANSMISSION_EXTENSIONS: &[&str] = &["m", "raw", "aux", "epc", "pwb", "uct", "cdf"];

/// Extensions (lowercase) that identify a distribution case file. `.pwd` is
/// the PowerWorld display sibling with no case data and stays excluded.
pub const DISTRIBUTION_EXTENSIONS: &[&str] = &["dss", "glm"];

/// `.json` carries no family signal; the shared JSON shape classifier decides.
const JSON_EXTENSION: &str = "json";

/// Extension list for error and empty-state messages; a unit test keeps it in
/// sync with the constants above.
pub const CASE_EXTENSIONS_LABEL: &str = ".m, .raw, .aux, .epc, .pwb, .uct, .cdf, .json, .dss, .glm";

/// Infer the case family from clear extensions, for `.json` the shared JSON
/// shape classifier, and for `.xml` files and directories the distribution
/// CIM check. `Some(true)` is distribution, `Some(false)` is transmission, and
/// `None` means the input carries no family signal.
pub fn infer_input_family(input: &Path) -> anyhow::Result<Option<bool>> {
    let ext = input
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    if looks_like_cim_dist_input(input, ext.as_deref()) {
        return Ok(Some(true));
    }
    let Some(ext) = ext.as_deref() else {
        return Ok(None);
    };
    if TRANSMISSION_EXTENSIONS.contains(&ext) {
        return Ok(Some(false));
    }
    if DISTRIBUTION_EXTENSIONS.contains(&ext) {
        return Ok(Some(true));
    }
    Ok(classified_json(input)?.map(|case| case.is_distribution()))
}

/// Whether a `.xml` file, or a directory holding one at its top level, reads
/// as distribution CIM.
fn looks_like_cim_dist_input(input: &Path, ext: Option<&str>) -> bool {
    cim_dist_source_with_ext(input, ext).is_some()
}

/// The opened source of a `.xml` file, or of a directory holding one at its
/// top level, that reads as distribution CIM. Each file's first 16 KiB settle
/// the header checks first, so a CGMES or XIIDM input is not read in full here
/// before its own parse reads it. The source keeps the documents the check
/// read, so the parse that takes it reads no file twice.
pub fn cim_dist_source(input: &Path) -> Option<powerio_core::Source> {
    let ext = input
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    cim_dist_source_with_ext(input, ext.as_deref())
}

fn cim_dist_source_with_ext(input: &Path, ext: Option<&str>) -> Option<powerio_core::Source> {
    let candidate = if input.is_dir() {
        std::fs::read_dir(input).is_ok_and(|entries| {
            entries.flatten().any(|entry| {
                let path = entry.path();
                path.is_file()
                    && path
                        .extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|e| e.eq_ignore_ascii_case("xml"))
                    && cim_dist_head(&path)
            })
        })
    } else {
        ext == Some("xml") && cim_dist_head(input)
    };
    if !candidate {
        return None;
    }
    let source = powerio_core::Source::open(input).ok()?;
    powerio_dist::source_looks_like_cim_dist(&source).then_some(source)
}

/// Whether a file's first 16 KiB open like a distribution CIM document.
fn cim_dist_head(path: &Path) -> bool {
    use std::io::Read;
    let Ok(file) = std::fs::File::open(path) else {
        return false;
    };
    let mut head = Vec::with_capacity(powerio_dist::CIM_DIST_HEAD_BYTES);
    if file
        .take(powerio_dist::CIM_DIST_HEAD_BYTES as u64)
        .read_to_end(&mut head)
        .is_err()
    {
        return false;
    }
    powerio_dist::looks_like_cim_dist_header(&head)
}

/// A `.json` case read and classified in one pass. The text rides along so the
/// typed parse reuses it instead of reading the file a second time.
pub struct ClassifiedCase {
    pub text: String,
    pub format: DetectedFormat,
}

impl ClassifiedCase {
    pub fn is_distribution(&self) -> bool {
        matches!(self.format, DetectedFormat::Distribution(_))
    }
}

/// The PowerIO IR document text when `input` is one, `Ok(None)` for anything
/// else. Single file commands send these to `deserialize` instead of the grid
/// exchange format classifier.
pub fn powerio_ir_text(input: &Path) -> anyhow::Result<Option<String>> {
    let ext = input
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    if ext.as_deref() != Some(JSON_EXTENSION) {
        return Ok(None);
    }
    let text = std::fs::read_to_string(input)
        .with_context(|| format!("reading JSON format markers from {}", input.display()))?;
    if matches!(
        powerio_tx::format::routing::classify_json_text(&text),
        JsonClass::Module
    ) {
        return Ok(Some(text));
    }
    Ok(None)
}

/// Read and classify `input` when it is a `.json` file; `Ok(None)` for every
/// other extension, whose family (if any) is named without touching the file.
/// The single-file routes hand the returned text straight to the typed parser
/// — the read-once rule #260 established for `batch` and the TUI.
pub fn classified_json(input: &Path) -> anyhow::Result<Option<ClassifiedCase>> {
    let ext = input
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    if ext.as_deref() != Some(JSON_EXTENSION) {
        return Ok(None);
    }
    let text = std::fs::read_to_string(input)
        .with_context(|| format!("reading JSON format markers from {}", input.display()))?;
    let format = classify_case_json(&text, input)?;
    // Either header verdict is provisional: an AssetInfo container alone
    // does not make a balanced document multiconductor, and per-phase
    // records may be nested below the top level.
    let format = match format {
        DetectedFormat::Transmission(
            powerio_tx::format::routing::TransmissionFormat::RavensJson,
        )
        | DetectedFormat::Distribution(
            powerio_tx::format::routing::DistributionFormat::RavensJson,
        ) => match powerio_core::cim::ravens::detect_profile(&text) {
            powerio_core::cim::ravens::Profile::Multiconductor => DetectedFormat::Distribution(
                powerio_tx::format::routing::DistributionFormat::RavensJson,
            ),
            powerio_core::cim::ravens::Profile::Mixed => {
                return Err(powerio_core::Error::new(
                    &powerio::tnd::diagnostics::PARSE_RAVENS_MIXED_PROFILE,
                    format!("{} carries both MG-RAVENS profiles", input.display()),
                )
                .into());
            }
            _ => DetectedFormat::Transmission(
                powerio_tx::format::routing::TransmissionFormat::RavensJson,
            ),
        },
        other => other,
    };
    if !matches!(
        format,
        DetectedFormat::Transmission(_) | DetectedFormat::Distribution(_)
    ) {
        return Err(crate::cli_failure(
            &crate::codes::REQUEST_CLI_OPTION_INVALID,
            format!(
                "unrecognized JSON format family `{}` in {}; pass --from to choose a format",
                format.name(),
                input.display()
            ),
        ));
    }
    Ok(Some(ClassifiedCase { text, format }))
}

/// Classify `.json` case text to its detected format, turning the non-case
/// outcomes (package, ambiguous markers, no markers) into errors
/// that name the fix.
fn classify_case_json(text: &str, path: &Path) -> anyhow::Result<DetectedFormat> {
    let refuse =
        |message: String| crate::cli_failure(&crate::codes::REQUEST_CLI_OPTION_INVALID, message);
    match powerio_tx::format::routing::classify_json_text(text) {
        JsonClass::Case(Detection::Known(format)) => Ok(format),
        JsonClass::Module => Err(refuse(format!(
            "{} is PowerIO IR; `summary`, `convert`, `verify`, `dcopf`, and \
             `sensitivities` read it directly, and `deserialize` loads it in the \
             language APIs",
            path.display()
        ))),
        JsonClass::Case(Detection::Ambiguous) => Err(refuse(format!(
            "ambiguous JSON markers in {}; pass --from to choose a format",
            path.display()
        ))),
        JsonClass::Case(Detection::Unknown) => Err(refuse(format!(
            "cannot infer JSON format for {}; pass --from to choose a format",
            path.display()
        ))),
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
    pub network: BalancedNetwork,
    pub warnings: Vec<String>,
}

/// Load one case file as a transmission [`BalancedNetwork`]. Distribution inputs
/// (`.dss`, BMOPF/PMD `.json`) parse to the multiconductor model and go
/// through the explicit balanced lowering pass, whose approximations and
/// dropped fields surface as warnings. A `.dss` redirect fragment (no voltage
/// source) fails the lowering preflight, so a recursive scan skips it instead
/// of exporting a partial feeder.
///
/// A `.json` case is read and classified once; the classifier's verdict names
/// the exact format, so the typed parse is the only other pass over the text.
pub fn load_network(path: &Path) -> anyhow::Result<LoadedCase> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    let stem = path.file_stem().and_then(|s| s.to_str());
    match ext.as_deref() {
        Some(JSON_EXTENSION) => {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("reading {}", path.display()))?;
            match classify_case_json(&text, path)? {
                DetectedFormat::Distribution(format) => {
                    let parsed = crate::module_io::load_multiconductor_memory(&text, format.name())
                        .with_context(|| format!("parse {}", path.display()))?;
                    lower_to_balanced(parsed, stem, path)
                }
                DetectedFormat::Transmission(format) => {
                    let parsed =
                        crate::module_io::load_balanced_memory_named(&text, format.name(), stem)
                            .with_context(|| format!("parse {}", path.display()))?;
                    Ok(LoadedCase {
                        warnings: powerio_core::render_diagnostics(parsed.diagnostics()),
                        network: parsed.into_value(),
                    })
                }
                other => anyhow::bail!(
                    "unrecognized JSON format family `{}` in {}; pass --from to choose a format",
                    other.name(),
                    path.display()
                ),
            }
        }
        Some(ext) if DISTRIBUTION_EXTENSIONS.contains(&ext) => {
            let parsed = crate::module_io::load_multiconductor_module(path, None)
                .with_context(|| format!("parse {}", path.display()))?;
            lower_to_balanced(parsed, stem, path)
        }
        Some(ext) if TRANSMISSION_EXTENSIONS.contains(&ext) => {
            let parsed = crate::module_io::load_balanced_module(path, None)
                .with_context(|| format!("parse {}", path.display()))?;
            Ok(LoadedCase {
                warnings: powerio_core::render_diagnostics(parsed.diagnostics()),
                network: parsed.into_value(),
            })
        }
        _ => anyhow::bail!("cannot infer a case format for {}", path.display()),
    }
}

/// Lower a parsed multiconductor network to the balanced model. A nameless
/// case takes its file stem as the network name first (the role the stem
/// plays for transmission formats); otherwise every nameless case in a batch
/// lowers to the same `lowered-multiconductor` fallback and the exports
/// overwrite each other.
fn lower_to_balanced(
    parsed: powerio_core::PioModule<powerio_dist::MulticonductorNetwork>,
    stem: Option<&str>,
    path: &Path,
) -> anyhow::Result<LoadedCase> {
    let mut warnings = powerio_core::render_diagnostics(parsed.diagnostics());
    let mut net = parsed.into_value();
    if net.name().is_none() {
        *net.name_mut() = stem.map(str::to_owned);
    }
    let lowered =
        to_balanced_network(&net, MulticonductorToBalancedOptions::default()).map_err(|e| {
            let diagnostics = e
                .diagnostics
                .iter()
                .map(powerio_core::Diagnostic::message)
                .collect::<Vec<_>>()
                .join("; ");
            if diagnostics.is_empty() {
                // Diagnostics should always be present on a refusal, but
                // an empty list must not render a bare trailing colon.
                anyhow::anyhow!("lower {} to balanced: {e}", path.display())
            } else {
                anyhow::anyhow!("lower {} to balanced: {diagnostics}", path.display())
            }
        })?;
    warnings.extend(powerio_core::render_diagnostics(&lowered.diagnostics));
    Ok(LoadedCase {
        network: lowered.network,
        warnings,
    })
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
        assert_eq!(
            discover_cases(&tmp.path().join("nope"), None),
            [] as [std::path::PathBuf; 0]
        );
    }

    #[test]
    fn nameless_distribution_json_takes_the_file_stem_name() {
        let dss = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../tests/data/dist/micro/fourwire_linecode.dss"
        );
        let source = powerio_core::Source::open(dss).unwrap();
        let module = powerio::dist::parse(source).unwrap();
        let emission = crate::module_io::emit_multiconductor_module(
            &module,
            powerio_dist::DistTargetFormat::BmopfJson,
        )
        .unwrap();
        let mut doc: serde_json::Value = serde_json::from_str(&emission.text).unwrap();
        doc.as_object_mut().unwrap().remove("name");
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("myfeeder.json");
        std::fs::write(&path, doc.to_string()).unwrap();
        let loaded = load_network(&path).unwrap();
        assert_eq!(loaded.network.name(), "myfeeder");
    }

    #[test]
    fn ravens_family_uses_semantic_profile_for_both_header_verdicts() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("case.json");
        for (text, distribution) in [
            (
                r#"{"ConnectivityNode":{},"AssetInfo":{},"MySettings":{"Ravens.cimObjectType":"AlgorithmSettings"}}"#,
                false,
            ),
            (
                r#"{"ConnectivityNode":{"TransformerTank":{"IdentifiedObject.description":"TransformerTank"}},"AssetInfo":{}}"#,
                false,
            ),
            (
                r#"{"ConnectivityNode":{},"nested":{"Ravens.cimObjectType":"EnergyConsumer\u0050hase"}}"#,
                true,
            ),
        ] {
            std::fs::write(&path, text).unwrap();
            let classified = classified_json(&path).unwrap().unwrap();
            assert_eq!(classified.is_distribution(), distribution, "{text}");
        }
        std::fs::write(
            &path,
            r#"{"ConnectivityNode":{},"PerLengthLineParameter":{},"MySettings":{"Ravens.cimObjectType":"AlgorithmSettings"}}"#,
        )
        .unwrap();
        let error = classified_json(&path)
            .err()
            .expect("mixed profiles are refused");
        assert!(error.to_string().contains("PARSE.RAVENS.MIXED_PROFILE"));
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
