use powerio::{
    ArtifactPath, Collection, CollectionLimits, CollectionSelection, ParseOptions, PioValue,
};
use std::sync::Arc;
fn collection(files: &[(&str, &[u8])]) -> Collection {
    Collection::from_files(
        "groups",
        files
            .iter()
            .map(|(p, b)| (ArtifactPath::new(*p).unwrap(), Arc::<[u8]>::from(*b))),
        CollectionLimits::default(),
    )
    .unwrap()
}
#[test]
fn go_roles_are_explicit_and_unrelated_json_cannot_be_paired() {
    let c = collection(&[
        (
            "p.json",
            include_bytes!("../../tests/data/goc3/goc3_small.json"),
        ),
        (
            "s.json",
            include_bytes!("../../tests/data/goc3/goc3_small_solution.json"),
        ),
        (
            "other/p.json",
            include_bytes!("../../tests/data/goc3/goc3_small.json"),
        ),
        ("broken.json", b"not json"),
    ]);
    let selection = CollectionSelection::group("goc3-json", vec!["p.json".into(), "s.json".into()]);
    let mut alias = selection.clone();
    alias.format = Some("GOC3-JSON".into());
    assert_eq!(
        c.source(&alias).unwrap().format().unwrap().as_str(),
        "goc3-json"
    );
    let result = c.parse(&selection, &ParseOptions::default()).unwrap();
    assert!(matches!(result.module.value(), PioValue::AcScucSolution(_)));
    let instance = c
        .parse(
            &CollectionSelection::file("p.json").format("goc3-json"),
            &ParseOptions::default(),
        )
        .unwrap();
    assert!(matches!(
        instance.module.value(),
        PioValue::AcScucInstance(_)
    ));
    for members in [
        vec!["s.json".into()],
        vec!["p.json".into(), "other/p.json".into()],
        vec!["p.json".into(), "p.json".into()],
    ] {
        assert!(
            c.parse(
                &CollectionSelection::group("goc3-json", members),
                &ParseOptions::default()
            )
            .is_err()
        );
    }
}
#[test]
fn selected_xml_groups_exclude_other_networks() {
    let c = collection(&[
        (
            "tx/EQ.xml",
            include_bytes!("../../tests/data/cgmes/node-breaker/NodeBreaker_EQ.xml"),
        ),
        (
            "tx/SSH.xml",
            include_bytes!("../../tests/data/cgmes/node-breaker/NodeBreaker_SSH.xml"),
        ),
        (
            "dist/feeder.xml",
            include_bytes!("../../tests/data/dist/cim/micro_feeder.xml"),
        ),
        ("dist/unrelated/invalid.xml", b"invalid"),
    ]);
    let mut tx = CollectionSelection::group("cgmes", vec!["EQ.xml".into(), "SSH.xml".into()]);
    tx.root = Some("tx".into());
    assert!(matches!(
        c.parse(&tx, &ParseOptions::default())
            .unwrap()
            .module
            .value(),
        PioValue::BalancedNetwork(_)
    ));
    let mut dist = CollectionSelection::group("cim-dist", vec!["feeder.xml".into()]);
    dist.root = Some("dist".into());
    assert!(matches!(
        c.parse(&dist, &ParseOptions::default())
            .unwrap()
            .module
            .value(),
        PioValue::MulticonductorNetwork(_)
    ));
    dist.members = Some(vec!["unrelated/invalid.xml".into()]);
    assert!(c.source(&dist).is_err());
}
#[test]
fn native_zip_reaches_the_adapter_intact_without_outer_companions() {
    let c = collection(&[
        ("native.sinx", b"broken native container"),
        ("external.db", b"outer bytes"),
    ]);
    let selection = CollectionSelection::file("native.sinx").format("sincal-balanced");
    let source = c.source(&selection).unwrap();
    assert_eq!(source.acquired_buffers().len(), 1);
    assert!(source.root_buffer("external.db").is_err());
    assert!(c.parse(&selection, &ParseOptions::default()).is_err());
}
