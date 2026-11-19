use std::sync::Arc;

use powerio::{
    ArtifactPath, Destination, EmittedOutput, Fidelity, PioValue, Source, SourceTree,
    SourceTreeLimits,
};

fn path(name: &str) -> ArtifactPath {
    ArtifactPath::new(name).unwrap()
}

#[test]
fn shared_dss_masters_match_legacy_parsing_without_sharing_parser_state() {
    let files = [
        (
            "masters/day.dss",
            "New Circuit.feeder basekv=12.47 bus1=source\nRedirect ../shared/topology.inc\nNew Load.day bus1=load phases=3 conn=wye kv=12.47 kw=30 kvar=6\n",
        ),
        (
            "masters/night.dss",
            "New Circuit.feeder basekv=12.47 bus1=source\nRedirect ../shared/topology.inc\nNew Load.night bus1=load phases=3 conn=wye kv=12.47 kw=15 kvar=3\n",
        ),
        (
            "shared/topology.inc",
            "New Line.l bus1=source bus2=load phases=3 r1=0.1 x1=0.2 r0=0.3 x0=0.4 length=1\n",
        ),
        (
            "notes.asset",
            "Retained companion, not a consumed dependency.\n",
        ),
    ];
    let buffers = || {
        files
            .iter()
            .map(|(name, text)| (path(name), Arc::<[u8]>::from(text.as_bytes())))
    };
    let tree = SourceTree::new("study", buffers(), SourceTreeLimits::default()).unwrap();
    for order in [[0, 1], [1, 0]] {
        let selections = [0, 1].map(|index| {
            tree.source(
                None,
                Some(&path(files[index].0)),
                SourceTreeLimits::default(),
            )
            .unwrap()
        });
        for index in order {
            let source = &selections[index];
            assert!(source.dependencies().is_empty());
            let module = powerio::parse(source).unwrap();
            let PioValue::MulticonductorNetwork(network) = module.value() else {
                panic!("distribution network expected")
            };
            assert_eq!(network.loads().len(), 1);
            assert_eq!(
                network.loads()[0].name,
                if index == 0 { "day" } else { "night" }
            );
            let legacy =
                Source::from_memory_tree("study", buffers(), Some(path(files[index].0))).unwrap();
            let legacy_module = powerio::parse(legacy).unwrap();
            let PioValue::MulticonductorNetwork(legacy_network) = legacy_module.value() else {
                panic!("distribution network expected")
            };
            assert_eq!(
                serde_json::to_value(network).unwrap(),
                serde_json::to_value(legacy_network).unwrap()
            );
            assert_eq!(
                serde_json::to_value(module.diagnostics()).unwrap(),
                serde_json::to_value(legacy_module.diagnostics()).unwrap()
            );
            assert_eq!(
                source
                    .dependencies()
                    .iter()
                    .map(powerio::SourceBuffer::name)
                    .collect::<Vec<_>>(),
                ["shared/topology.inc"]
            );
            let output =
                powerio::emit(&module, "dss", Destination::memory("copy").unwrap()).unwrap();
            assert_eq!(output.fidelity(), Fidelity::ExactSameFormat);
            let EmittedOutput::Memory { artifacts } = output.output() else {
                panic!("memory output expected")
            };
            for (name, text) in files {
                assert!(artifacts.iter().any(|artifact| artifact.name().as_str()
                    == format!("copy/source/{name}")
                    && artifact.bytes() == text.as_bytes()));
            }
            // Replay enumerates all companions but does not observe reads.
            assert_eq!(source.dependencies().len(), 1);
        }
    }
}

#[test]
fn rebased_pypsa_directory_view_excludes_other_datasets_and_preserves_echo() {
    let files = [
        ("study/network.csv", "name\nexample\n"),
        ("study/buses.csv", "name,v_nom\nB1,138.0\nB2,138.0\n"),
        ("study/loads.csv", "name,bus,p_set,q_set\nL1,B2,5.0,1.0\n"),
        (
            "study/generators.csv",
            "name,bus,control,p_nom,p_set\nG1,B1,Slack,100.0,12.0\n",
        ),
        ("other/network.csv", "name\nother\n"),
    ];
    let tree = SourceTree::new(
        "batch",
        files
            .iter()
            .map(|(name, text)| (path(name), Arc::<[u8]>::from(text.as_bytes()))),
        SourceTreeLimits::default(),
    )
    .unwrap();
    let source = tree
        .source(Some(&path("study")), None, SourceTreeLimits::default())
        .unwrap();
    let module = powerio::parse(&source).unwrap();
    let PioValue::BalancedNetwork(network) = module.value() else {
        panic!("balanced network expected")
    };
    assert_eq!(network.buses().len(), 2);
    assert_eq!(source.acquired_buffers().len(), 4);
    assert!(
        source
            .dependencies()
            .iter()
            .all(|buffer| !buffer.name().starts_with("other/"))
    );
    let output = powerio::emit(&module, "pypsa-csv", Destination::memory("copy").unwrap()).unwrap();
    assert_eq!(output.fidelity(), Fidelity::ExactSameFormat);
    let EmittedOutput::Memory { artifacts } = output.output() else {
        panic!("memory output expected")
    };
    assert_eq!(artifacts.len(), 4);
    for (name, text) in &files[..4] {
        let relative = name.strip_prefix("study/").unwrap();
        assert!(
            artifacts
                .iter()
                .any(|artifact| artifact.name().as_str().ends_with(relative)
                    && artifact.bytes() == text.as_bytes())
        );
    }
}
