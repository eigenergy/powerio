use std::sync::Arc;

use powerio::{ArtifactPath, Destination, EmittedOutput, Fidelity, PioValue, Source};

fn tree(files: &[(&str, &str)], primary: Option<&str>) -> Source {
    Source::from_memory_tree(
        "project",
        files.iter().map(|(name, text)| {
            (
                ArtifactPath::new(*name).unwrap(),
                Arc::<[u8]>::from(text.as_bytes()),
            )
        }),
        primary.map(|name| ArtifactPath::new(name).unwrap()),
    )
    .unwrap()
}

#[test]
fn pypsa_memory_directory_parses_and_echoes_every_file() {
    let files = [
        ("network.csv", "name\nexample\n"),
        ("buses.csv", "name,v_nom\nB1,138.0\nB2,138.0\n"),
        ("loads.csv", "name,bus,p_set,q_set\nL1,B2,5.0,1.0\n"),
        (
            "generators.csv",
            "name,bus,control,p_nom,p_set\nG1,B1,Slack,100.0,12.0\n",
        ),
    ];
    let source = tree(&files, None);
    assert!(source.is_directory());
    assert!(source.primary_buffer().is_err());
    let module = powerio::parse(source).unwrap();
    let PioValue::BalancedNetwork(network) = module.value() else {
        panic!("network expected")
    };
    assert_eq!(network.buses().len(), 2);
    let result = powerio::emit(&module, "pypsa-csv", Destination::memory("out").unwrap()).unwrap();
    assert_eq!(result.fidelity(), Fidelity::ExactSameFormat);
    let EmittedOutput::Memory { artifacts } = result.output() else {
        panic!("memory expected")
    };
    assert_eq!(artifacts.len(), files.len());
    for (name, bytes) in files {
        let artifact = artifacts
            .iter()
            .find(|a| a.name().as_str().ends_with(name))
            .unwrap();
        assert_eq!(artifact.bytes(), bytes.as_bytes());
    }
}

#[test]
fn nested_dss_primary_resolves_sibling_and_parent_paths() {
    let files = [
        (
            "model/Master.dss",
            "Clear\nNew Circuit.example basekv=12.47 bus1=source\nRedirect lines.dss\nRedirect ../loads.dss\n",
        ),
        (
            "model/lines.dss",
            "New Line.l bus1=source bus2=load phases=3 r1=0.1 x1=0.2 r0=0.3 x0=0.4 length=1\n",
        ),
        (
            "loads.dss",
            "New Load.ld bus1=load phases=3 conn=wye kv=12.47 kw=10 kvar=2\n",
        ),
    ];
    let source = tree(&files, Some("model/Master.dss"));
    assert!(!source.is_directory());
    assert_eq!(source.name(), "model/Master.dss");
    let primary = source.primary_buffer().unwrap();
    assert_eq!(
        source
            .referenced_buffer(&primary, "../loads.dss")
            .unwrap()
            .bytes(),
        files[2].1.as_bytes()
    );
    assert!(
        source
            .referenced_buffer(&primary, "../../outside.dss")
            .is_err()
    );
    let module = powerio::parse(source).unwrap();
    assert!(matches!(module.value(), PioValue::MulticonductorNetwork(_)));
    assert_eq!(module.sources().len(), 3);
    powerio::emit(
        &module,
        "pmd-json",
        Destination::memory("case.json").unwrap(),
    )
    .unwrap();
}

#[test]
fn trees_refuse_duplicate_missing_and_over_budget_files() {
    let files = || {
        [(
            ArtifactPath::new("a").unwrap(),
            Arc::<[u8]>::from(&b"x"[..]),
        )]
    };
    assert!(Source::from_memory_tree("project", files().into_iter().chain(files()), None).is_err());
    assert!(
        Source::from_memory_tree(
            "project",
            files(),
            Some(ArtifactPath::new("missing").unwrap())
        )
        .is_err()
    );
    let too_many = (0..4097).map(|n| {
        (
            ArtifactPath::new(format!("file{n}")).unwrap(),
            Arc::<[u8]>::from(&b""[..]),
        )
    });
    assert!(Source::from_memory_tree("project", too_many, None).is_err());
    let deep = format!("{}a", "d/".repeat(64));
    assert!(
        Source::from_memory_tree(
            "project",
            [(
                ArtifactPath::new(deep).unwrap(),
                Arc::<[u8]>::from(&b""[..])
            )],
            None
        )
        .is_err()
    );
}

#[test]
fn format_catalog_tokens_resolve_and_read_only_targets_stay_read_only() {
    let catalog: Vec<_> = powerio::grid_formats().collect();
    for entry in &catalog {
        assert_eq!(
            powerio::resolve_format(entry.format.token),
            Some(entry.format)
        );
    }
    for token in ["pwb", "ieee-cdf", "opfdata-json"] {
        assert!(
            !catalog
                .iter()
                .find(|entry| entry.format.token == token)
                .unwrap()
                .format
                .can_emit
        );
    }
    assert!(
        catalog
            .iter()
            .any(|entry| entry.format.token == "bmopf-json@0.2.0")
    );
}
