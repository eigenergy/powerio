use std::path::Path;

use powerio::{PioValue, Source};

#[test]
fn parses_the_pinned_goc3_benchmark_cases() {
    let Ok(root) = std::env::var("POWERIO_GOC3_REFERENCE_DATA") else {
        eprintln!(
            "POWERIO_GOC3_REFERENCE_DATA is unset; the pinned reference-data CI job runs this test"
        );
        return;
    };
    let paths = reference_paths(Path::new(&root));
    assert_eq!(
        paths.len(),
        3,
        "the pinned GOC3Benchmark revision has D1, D2, and D3"
    );

    for path in paths {
        assert_benchmark_case(&path);
    }
}

fn reference_paths(root: &Path) -> Vec<std::path::PathBuf> {
    let mut paths: Vec<_> = std::fs::read_dir(root)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", root.display()))
        .map(|entry| entry.expect("reference-data directory entry").path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect();
    paths.sort();
    paths
}

fn assert_benchmark_case(path: &Path) {
    let module = powerio::parse(Source::open(path).unwrap(), Some("goc3"))
        .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()));
    let PioValue::AcScucInstance(instance) = &module.value else {
        panic!("{} did not produce powerio.AcScucInstance", path.display());
    };
    let name = path.file_name().unwrap().to_string_lossy();
    let expected_periods = if name.contains("D1_") {
        18
    } else if name.contains("D2_") {
        48
    } else if name.contains("D3_") {
        42
    } else {
        panic!("unexpected pinned benchmark file {name}");
    };
    assert_eq!(instance.network().buses().len(), 73, "{}", path.display());
    assert_eq!(
        instance.network().branches().len(),
        120,
        "{}",
        path.display()
    );
    assert_eq!(instance.network().hvdc().len(), 1, "{}", path.display());
    assert_eq!(instance.inputs().devices.len(), 205, "{}", path.display());
    assert_eq!(instance.inputs().shunts.len(), 73, "{}", path.display());
    assert_eq!(
        instance.inputs().branch_switching_costs.len(),
        120,
        "{}",
        path.display()
    );
    assert_eq!(
        instance.inputs().transformer_controls.len(),
        15,
        "{}",
        path.display()
    );
    assert_eq!(
        instance.inputs().active_reserve_zones.len(),
        1,
        "{}",
        path.display()
    );
    assert_eq!(
        instance.inputs().reactive_reserve_zones.len(),
        1,
        "{}",
        path.display()
    );
    assert_eq!(
        instance.inputs().contingencies.len(),
        2,
        "{}",
        path.display()
    );
    assert_eq!(
        instance.inputs().interval_durations.len(),
        expected_periods,
        "{}",
        path.display()
    );
    assert!(
        instance
            .inputs()
            .devices
            .iter()
            .all(|device| device.periods.len() == expected_periods),
        "{}",
        path.display()
    );
    let compatibility: Vec<_> = module
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code() == "READ.GOC3.RETAINED_SOURCE_ONLY")
        .collect();
    assert_eq!(
        compatibility.len(),
        1,
        "{}: {compatibility:?}",
        path.display()
    );
    assert!(
        compatibility[0]
            .message()
            .contains("con_loss_factor retained in source only for 73 buses"),
        "{}: {compatibility:?}",
        path.display()
    );
}
