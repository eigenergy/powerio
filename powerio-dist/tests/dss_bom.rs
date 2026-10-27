//! Byte order mark handling across the DSS read paths: the mark is retained
//! on the primary buffer for the byte exact echo, the reader decodes a mark
//! free slice, and a redirected file's mark never reaches the tokenizer.

mod helpers;

#[test]
fn byte_order_marks_are_retained_and_never_reach_the_reader() {
    let tmp = tempfile::tempdir().unwrap();
    let linecodes = tmp.path().join("linecodes.dss");
    std::fs::write(
        &linecodes,
        "\u{feff}new linecode.lc1 nphases=3 r1=0.1 x1=0.2\n",
    )
    .unwrap();
    let master_text = format!(
        "\u{feff}clear\nnew circuit.c basekv=12.47 bus1=src\nredirect {}\n",
        linecodes.display()
    );
    let master = tmp.path().join("master.dss");
    std::fs::write(&master, &master_text).unwrap();

    let net = helpers::parse_dss_file(&master).unwrap();
    // Retaining the mark is not a loss, so nothing warns about it.
    assert!(
        !net.warnings.iter().any(|w| w.contains("byte order mark")),
        "warnings: {:?}",
        net.warnings
    );
    // The linecode from the redirected file parsed: its own mark was skipped
    // by the decode slice, never tokenized into the first command word.
    assert!(net.line_codes().iter().any(|lc| lc.name == "lc1"));
    let output = net.emit(powerio_dist::DistTargetFormat::Dss);
    assert_eq!(output.text, "Redirect \"source/master.dss\"\n");
    let retained = output
        .sidecars
        .iter()
        .find(|file| file.path == "source/master.dss")
        .unwrap();
    assert_eq!(retained.text, master_text);
    let dependency = output
        .sidecars
        .iter()
        .find(|file| file.path == "source/linecodes.dss")
        .unwrap();
    assert_eq!(
        dependency.text,
        "\u{feff}new linecode.lc1 nphases=3 r1=0.1 x1=0.2\n"
    );
}
