//! Byte order mark handling across the DSS read paths: the strip must be
//! itemized for the root file and for every redirected file.

#[test]
fn redirected_file_bom_strip_is_itemized() {
    let tmp = tempfile::tempdir().unwrap();
    let linecodes = tmp.path().join("linecodes.dss");
    std::fs::write(
        &linecodes,
        "\u{feff}new linecode.lc1 nphases=3 r1=0.1 x1=0.2\n",
    )
    .unwrap();
    let master = tmp.path().join("master.dss");
    std::fs::write(
        &master,
        format!(
            "\u{feff}clear\nnew circuit.c basekv=12.47 bus1=src\nredirect {}\n",
            linecodes.display()
        ),
    )
    .unwrap();

    let net = powerio_dist::parse_dss_file(&master).unwrap();
    let bom_warnings: Vec<_> = net
        .warnings
        .iter()
        .filter(|w| w.contains("byte order mark"))
        .collect();
    // One warning for the root file, one naming the redirected file.
    assert_eq!(bom_warnings.len(), 2, "warnings: {:?}", net.warnings);
    assert!(
        bom_warnings.iter().any(|w| w.contains("linecodes.dss")),
        "warnings: {:?}",
        net.warnings
    );
    // The linecode from the redirected file still parsed.
    assert!(net.linecodes.iter().any(|lc| lc.name == "lc1"));
}
