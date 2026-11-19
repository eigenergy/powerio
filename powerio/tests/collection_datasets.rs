use powerio::collection::CandidateStatus;
use powerio::{ArtifactPath, Collection, CollectionLimits, ParseOptions, PioValue};
use std::sync::Arc;

#[test]
fn pypsa_roots_are_rebased_and_internal_snapshots_survive() {
    let mut files = Vec::new();
    for root in ["day", "night"] {
        for (leaf, text) in [
            ("network.csv", "name\nexample\n"),
            ("buses.csv", "name,v_nom\nB1,138\nB2,138\n"),
            ("loads.csv", "name,bus,p_set,q_set\nL1,B2,5,1\n"),
            (
                "generators.csv",
                "name,bus,control,p_nom,p_set\nG1,B1,Slack,100,5\n",
            ),
            (
                "snapshots.csv",
                "snapshot\n2020-01-01 00:00:00\n2020-01-01 01:00:00\n",
            ),
            (
                "loads-p_set.csv",
                "snapshot,L1\n2020-01-01 00:00:00,5\n2020-01-01 01:00:00,6\n",
            ),
            ("notes.txt", "source-only notes"),
        ] {
            files.push((
                ArtifactPath::new(format!("{root}/{leaf}")).unwrap(),
                Arc::<[u8]>::from(text.as_bytes()),
            ));
        }
    }
    files.push((
        ArtifactPath::new("day/nested/network.csv").unwrap(),
        Arc::<[u8]>::from(&b"name\nnested\n"[..]),
    ));
    let collection =
        Collection::from_files("datasets", files, CollectionLimits::default()).unwrap();
    let candidates = collection.inspect().unwrap();
    let datasets: Vec<_> = candidates
        .iter()
        .filter(|c| c.selection.format.as_deref() == Some("pypsa-csv"))
        .collect();
    assert_eq!(datasets.len(), 3);
    assert_eq!(
        datasets
            .iter()
            .filter(|c| c.status == CandidateStatus::Incomplete)
            .count(),
        1
    );
    for candidate in datasets
        .iter()
        .filter(|c| c.status == CandidateStatus::Selectable)
    {
        let mut alias = candidate.selection.clone();
        alias.format = Some("PyPSA".into());
        assert_eq!(
            collection
                .source(&alias)
                .unwrap()
                .format()
                .unwrap()
                .as_str(),
            "pypsa-csv"
        );
        let source = collection.source(&candidate.selection).unwrap();
        assert_eq!(source.entry_names().unwrap().len(), 7);
        let result = collection
            .parse(&candidate.selection, &ParseOptions::default())
            .unwrap();
        assert!(matches!(result.module.value(), PioValue::TimeSeries(_)));
    }
}

#[test]
fn gridfm_is_discoverable_even_when_not_compiled_in() {
    let collection = Collection::from_files(
        "gridfm",
        ["a", "b"].into_iter().flat_map(|root| {
            [
                "bus_data.parquet",
                "gen_data.parquet",
                "branch_data.parquet",
            ]
            .into_iter()
            .map(move |leaf| {
                (
                    ArtifactPath::new(format!("{root}/raw/{leaf}")).unwrap(),
                    Arc::<[u8]>::from(&b"opaque"[..]),
                )
            })
        }),
        CollectionLimits::default(),
    )
    .unwrap();
    let candidates = collection.inspect().unwrap();
    assert_eq!(candidates.len(), 2);
    for candidate in candidates {
        assert_eq!(
            candidate.status,
            if cfg!(feature = "gridfm") {
                CandidateStatus::Selectable
            } else {
                CandidateStatus::UnsupportedBuild
            }
        );
        assert_eq!(candidate.selection.format.as_deref(), Some("gridfm"));
        if !cfg!(feature = "gridfm") {
            assert!(collection.source(&candidate.selection).is_err());
        }
    }
}

#[test]
#[cfg(feature = "gridfm")]
fn independent_gridfm_roots_keep_internal_scenario_sets() {
    use powerio_matrix::{GridfmOptions, GridfmSnapshot, emit_gridfm_batch};
    let module = powerio::parse(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tests/data/case9.m"
    ))
    .unwrap();
    let PioValue::BalancedNetwork(base) = module.value() else {
        panic!()
    };
    let mut varied = base.clone();
    for load in varied.loads_mut() {
        load.p *= 1.1;
    }
    let directory = tempfile::tempdir().unwrap();
    for name in ["first", "second"] {
        emit_gridfm_batch(
            &[
                GridfmSnapshot::new(base, 0),
                GridfmSnapshot::new(&varied, 1),
            ],
            directory.path().join(name),
            &GridfmOptions::default(),
        )
        .unwrap();
    }
    let collection =
        Collection::from_directory(directory.path(), CollectionLimits::default()).unwrap();
    let candidates = collection.inspect().unwrap();
    let datasets: Vec<_> = candidates
        .iter()
        .filter(|c| c.selection.format.as_deref() == Some("gridfm"))
        .collect();
    assert_eq!(datasets.len(), 2);
    for candidate in datasets {
        let parsed = collection
            .parse(&candidate.selection, &ParseOptions::default())
            .unwrap();
        let PioValue::ScenarioSet(scenarios) = parsed.module.value() else {
            panic!()
        };
        assert_eq!(scenarios.len(), 2);
    }
}
