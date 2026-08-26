//! The corpus harness, held to its two defining properties: it groups a case's
//! siblings whatever they are called, and nothing from a case reaches the
//! report.

use std::path::{Path, PathBuf};

use powerio_cli::corpus::{self, anonymize, fingerprint::Fingerprint};
use powerio_matrix::TargetFormat;

fn data(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("tests/data")
        .join(rel)
}

/// Write `case9.m` into several formats under deliberately misleading names,
/// so the only thing that can group them is the electrical content.
fn build_corpus(dir: &Path) {
    let net = parse_matpower_file(data("case9.m")).unwrap();
    for (target, name) in [
        (TargetFormat::Matpower, "CONFIDENTIAL_FeederAlpha_2019.m"),
        (TargetFormat::Psse { rev: 33 }, "utility_export_q3.raw"),
        (TargetFormat::EgretJson, "NDA_internal_model.json"),
    ] {
        let text = powerio_matrix::write_network(&net, target).unwrap().text;
        std::fs::write(dir.join(name), text).unwrap();
    }
    // A file no reader can make sense of. A corpus always has one.
    std::fs::write(dir.join("scratch_notes.raw"), "this is not a case\n").unwrap();
}

#[test]
fn siblings_group_by_content_whatever_they_are_called() {
    let work = tempfile::tempdir().unwrap();
    let corpus = tempfile::tempdir().unwrap();
    build_corpus(corpus.path());

    let ingest = corpus::ingest(corpus.path(), work.path(), None).unwrap();
    let grouped: Vec<_> = ingest
        .buckets
        .iter()
        .filter(|b| b.members.len() > 1)
        .collect();
    assert_eq!(
        grouped.len(),
        1,
        "the three spellings of one case belong in one bucket, got {:?}",
        ingest
            .buckets
            .iter()
            .map(|b| (b.id.clone(), b.members.len()))
            .collect::<Vec<_>>()
    );
    assert_eq!(grouped[0].members.len(), 3);
    assert!(
        grouped[0].id.starts_with("case-"),
        "bucket ids are ordinals, never anything from a filename: {}",
        grouped[0].id
    );
    assert_eq!(
        ingest.unreadable.len(),
        1,
        "the junk file is a finding, not a crash"
    );
}

#[test]
fn the_report_carries_nothing_from_the_corpus() {
    let work = tempfile::tempdir().unwrap();
    let corpus = tempfile::tempdir().unwrap();
    build_corpus(corpus.path());

    corpus::ingest(corpus.path(), work.path(), None).unwrap();
    corpus::compare(work.path()).unwrap();
    let findings = work.path().join("findings.jsonl");
    let summary = work.path().join("summary.md");
    corpus::report(work.path(), &findings, Some(&summary)).unwrap();

    let emitted = format!(
        "{}{}",
        std::fs::read_to_string(&findings).unwrap(),
        std::fs::read_to_string(&summary).unwrap()
    );
    for secret in [
        "CONFIDENTIAL",
        "FeederAlpha",
        "utility_export",
        "NDA_internal",
        "scratch_notes",
    ] {
        assert!(
            !emitted.contains(secret),
            "the report named a corpus file: {secret}"
        );
    }
    assert!(
        !emitted.contains(corpus.path().to_str().unwrap()),
        "the report named the corpus directory"
    );
    // The report still has to be worth reading.
    assert!(emitted.contains("case-"), "no buckets reached the report");
}

#[test]
fn a_planted_string_fails_the_audit() {
    let mut sanitizer = anonymize::Sanitizer::new();
    sanitizer.learn_path(Path::new("/archive/SUBSTATION_WESTFIELD.raw"));
    assert!(sanitizer.audit("bucket case-000: two rows differ").is_ok());
    let leaks = sanitizer
        .audit("bucket case-000: SUBSTATION_WESTFIELD lost a row")
        .unwrap_err();
    assert_eq!(leaks.len(), 1);
    assert_eq!(leaks[0].line, 1);
}

/// The audit prefilters each secret on the word run it opens with, so most are
/// excluded by a hash lookup instead of a scan. These are the shapes that
/// would break if the prefilter were a plain "is this secret one of the line's
/// tokens" test: a secret carrying a space or a dot spans several tokens, and
/// one that opens with a non-word character has no leading run to filter on.
#[test]
fn the_audit_still_catches_secrets_that_span_tokens() {
    let mut sanitizer = anonymize::Sanitizer::new();
    for secret in ["BUS A", "FEEDER.ALPHA", "-WESTFIELD", "PLAIN"] {
        sanitizer.learn_network(&serde_json::json!({ "name": secret }));
    }
    for (line, expected) in [
        ("row names BUS A here", "BUS A"),
        ("row names FEEDER.ALPHA here", "FEEDER.ALPHA"),
        ("row names -WESTFIELD here", "-WESTFIELD"),
        ("row names PLAIN here", "PLAIN"),
    ] {
        let leaks = sanitizer
            .audit(line)
            .expect_err(&format!("audit missed {expected} in {line:?}"));
        assert!(
            leaks.iter().any(|l| l.secret == expected),
            "audit missed {expected}: {leaks:?}"
        );
    }
    // A word run that only appears as part of a longer token is not a match,
    // exactly as before: `word_boundaries` still decides.
    assert!(sanitizer.audit("row names PLAINTEXT here").is_ok());
}

#[test]
fn a_format_token_is_vocabulary_even_when_the_corpus_uses_it_as_a_directory() {
    let mut sanitizer = anonymize::Sanitizer::new();
    sanitizer.learn_path(Path::new("/archive/psse/case.raw"));
    sanitizer.allow("psse");
    assert!(
        sanitizer
            .audit(r#"{"from":"psse","to":"matpower"}"#)
            .is_ok(),
        "a corpus laid out by format must not make the format unnameable"
    );
}

#[test]
fn a_case_with_dc_lines_is_not_the_ac_network_underneath_it() {
    let plain = parse_matpower_file(data("case9.m")).unwrap();
    let with_dc = parse_matpower_file(data("t_case9_dcline.m")).unwrap();
    assert_ne!(
        Fingerprint::of(&plain).primary(),
        Fingerprint::of(&with_dc).primary(),
        "a DC link changes the case, however alike the AC networks look"
    );
}

#[test]
fn service_status_stays_out_of_the_bucketing_key() {
    // Two files that differ only in what is switched out must land together,
    // because a reader that drops status is exactly what the sibling
    // comparison exists to catch.
    let base = parse_matpower_file(data("case9.m")).unwrap();
    let mut switched = base.clone();
    switched.branches[0].in_service = false;
    switched.generators[0].in_service = false;
    assert_eq!(
        Fingerprint::of(&base).primary(),
        Fingerprint::of(&switched).primary()
    );
    assert!(Fingerprint::of(&base).agrees_with(&Fingerprint::of(&switched)));
}

#[test]
fn paths_collapse_to_their_class() {
    assert_eq!(anonymize::collapse_path(".loads[3].p"), ".loads[#].p");
    assert_eq!(
        anonymize::collapse_path(".branches[47].extras.LineCircuit"),
        ".branches[#].extras.LineCircuit"
    );
}

#[test]
fn values_leave_as_magnitudes_rather_than_numbers() {
    assert_eq!(anonymize::magnitude(1475.69), Some(3));
    assert_eq!(anonymize::magnitude(0.004), Some(-3));
    assert_eq!(anonymize::magnitude(0.0), None);
    assert_eq!(anonymize::ratio(200.0, 100.0), Some(0.5));
    assert_eq!(anonymize::ratio(0.0, 100.0), None);
}

/// An `Extras` key is whatever token the source stated, so it is case data in a
/// position the report reaches through a serde path. Learning only values left
/// it unmasked and unaudited.
#[test]
fn an_extras_key_is_learned_like_any_other_name() {
    let mut sanitizer = anonymize::Sanitizer::new();
    let value = serde_json::json!({
        "loads": [{ "p": 1.0, "extras": { "AcmeUtility_FeederCode_88": 1 } }]
    });
    sanitizer.learn_network(&value);
    assert!(
        sanitizer
            .audit(".loads[#].extras.AcmeUtility_FeederCode_88")
            .is_err(),
        "an extras key must be auditable"
    );
    assert!(
        !sanitizer
            .template(".loads[#].extras.AcmeUtility_FeederCode_88")
            .contains("AcmeUtility"),
        "an extras key must be masked"
    );
    // powerio's own field names are vocabulary and survive, or every path in
    // the report would read as a redaction.
    assert_eq!(sanitizer.template(".loads[#].p"), ".loads[#].p");
}

/// A corpus string spelled like a powerio field name is vocabulary, so the
/// field stays reportable. Before #340 the vocabulary held only what the
/// reference network populated, and a finding about `charging.g_fr` read
/// `charging.<name>` the day a corpus taught the same spelling.
#[test]
fn a_field_name_the_corpus_also_spells_stays_reportable() {
    let mut sanitizer = anonymize::Sanitizer::new();
    sanitizer.learn_network(&serde_json::json!({
        "branches": [{ "extras": { "g_fr": 1.0, "band_min": 2.0 } }]
    }));
    assert_eq!(
        sanitizer.template(".branches[#].charging.g_fr"),
        ".branches[#].charging.g_fr"
    );
    assert!(
        sanitizer
            .audit(".transformers[#].bands[#].band_min")
            .is_ok()
    );
}

/// A corpus may hold a symlink whose name sits under the root and whose target
/// does not. The walk does not descend symlinked directories, but it still
/// hands over symlinked files, and reading one reads its target.
#[test]
fn a_symlink_out_of_the_corpus_is_not_read() {
    let outside = tempfile::tempdir().unwrap();
    let secret = outside.path().join("elsewhere.m");
    std::fs::write(&secret, std::fs::read_to_string(data("case9.m")).unwrap()).unwrap();

    let corpus = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    std::fs::write(
        corpus.path().join("inside.m"),
        std::fs::read_to_string(data("case9.m")).unwrap(),
    )
    .unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&secret, corpus.path().join("link.m")).unwrap();

    let ingest = corpus::ingest(corpus.path(), work.path(), None).unwrap();
    let members: usize = ingest.buckets.iter().map(|b| b.members.len()).sum();
    #[cfg(unix)]
    {
        assert_eq!(members, 1, "only the file that really lives here is read");
        assert_eq!(ingest.escaped, 1, "and the escape is counted");
    }
    #[cfg(not(unix))]
    assert_eq!(members, 1);
}

fn parse_matpower_file(
    path: impl AsRef<std::path::Path>,
) -> Result<powerio_matrix::BalancedNetwork, powerio_core::Error> {
    let source = powerio_core::Source::open(path.as_ref())?
        .with_format(powerio_core::FormatId::new("matpower")?);
    powerio_matrix::parse(source).map(powerio_core::PioModule::into_value)
}
