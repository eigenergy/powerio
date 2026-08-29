//! A stored source descriptor's name is a display name, not a filesystem
//! path: parsing a file at a path with a directory component must not leave
//! that directory in the descriptor the module retains. The descriptor also
//! carries the format the parser resolved.
use std::path::PathBuf;

mod helpers;
use helpers::parse_dss_file;

fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/data/dist")
        .join(rel)
}

#[test]
fn source_descriptor_name_drops_the_directory_and_carries_the_format() {
    let parsed = parse_dss_file(fixture("micro/switch.dss")).expect("fixture parses");
    let sources = parsed.module.sources();
    assert!(
        !sources.is_empty(),
        "a file source must retain a descriptor"
    );
    for source in sources {
        assert!(
            !source.name().contains('/') && !source.name().contains('\\'),
            "stored source name must not carry a directory: {}",
            source.name()
        );
    }
    assert_eq!(sources[0].name(), "switch.dss");
    assert_eq!(
        sources[0].format().map(powerio_core::FormatId::as_str),
        Some("dss")
    );
}
