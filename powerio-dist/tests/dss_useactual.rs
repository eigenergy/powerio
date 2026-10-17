use powerio_core::{Destination, EmittedOutput, Source};
use powerio_dist::DistTargetFormat;

fn source(text: &str) -> Source {
    Source::from_memory("case.dss", text.as_bytes()).unwrap()
}

#[test]
fn binary_profiles_are_not_text_and_same_format_keeps_every_byte() {
    // These bytes are a valid double near 7; the first three happen to be the
    // UTF-8 BOM. Binary acquisition must never strip them as a text marker.
    let bytes = vec![0xef, 0xbb, 0xbf, 0, 0, 0, 0x1c, 0x40];
    let p = f64::from_le_bytes(bytes.clone().try_into().unwrap());
    let text = "new circuit.c\nnew loadshape.s npts=1 mult=[dblfile=p.bin] useactual=yes\nnew load.l bus1=sourcebus kw=1 kvar=0 daily=s\n";
    let module = powerio_dist::parse(
        source(text)
            .with_named_buffer("p.bin", bytes.clone())
            .unwrap(),
    )
    .unwrap();
    assert!((module.value().loads()[0].p_nom.iter().sum::<f64>() / 1000.0 - p).abs() < 1e-12);
    let emitted = powerio_dist::emit(
        &module,
        DistTargetFormat::Dss,
        Destination::memory("out").unwrap(),
    )
    .unwrap();
    let EmittedOutput::Memory { artifacts } = emitted.into_output() else {
        panic!("memory output");
    };
    assert!(
        artifacts
            .iter()
            .any(|a| a.name().as_str() == "out/p.bin" && a.bytes() == bytes)
    );
    assert!(
        artifacts
            .iter()
            .any(|a| a.name().as_str() == "out/case.dss" && a.bytes() == text.as_bytes())
    );
}

#[test]
fn actual_shape_failures_cannot_leave_the_old_nominal_power_silently() {
    for shape in [
        "npts=3 mult=[dblfile=missing.bin]",
        "npts=3 mult=[file=../outside.csv]",
        "npts=1048577 pmax=7",
        "npts=3 mult=[5 7 3] action=normalize",
        "npts=3 mult=[5 7 3] qmult=[dblfile=short.bin]",
    ] {
        let text = format!(
            "new circuit.c\nnew loadshape.s useactual=yes {shape}\nnew load.l bus1=sourcebus kw=1 kvar=0 daily=s\n"
        );
        let input = source(&text)
            .with_named_buffer("short.bin", 2.0_f64.to_le_bytes().to_vec())
            .unwrap();
        assert!(powerio_dist::parse(input).is_err(), "{shape}");
    }
}

#[test]
fn shape_reference_captures_peak_at_assignment_and_respects_pf_precedence() {
    let text = "new circuit.c\nnew loadshape.s npts=3 mult=[5 7 3] qmult=[9 2 99] useactual=yes\nnew load.a bus1=sourcebus kw=4 kvar=1 daily=s\nedit loadshape.s mult=[10 20 30] qmult=[2 4 6]\nnew load.b like=a\nnew load.c bus1=sourcebus kw=4 pf=.8 daily=s\nnew load.d bus1=sourcebus kw=4 pf=.8 kvar=1 daily=s\n";
    let module = powerio_dist::parse(source(text)).unwrap();
    for (load, (p, q)) in
        module
            .value()
            .loads()
            .iter()
            .zip([(7.0, 2.0), (7.0, 2.0), (30.0, 22.5), (30.0, 6.0)])
    {
        assert!(
            (load.p_nom.iter().sum::<f64>() / 1000.0 - p).abs() < 1e-12,
            "{}",
            load.name
        );
        assert!(
            (load.q_nom.iter().sum::<f64>() / 1000.0 - q).abs() < 1e-12,
            "{}",
            load.name
        );
    }
}

#[test]
fn scalar_transformer_ratings_sync_but_array_ratings_remain_independent() {
    for (rating, expected) in [
        ("wdg=1 kva=75 wdg=2 kva=50", [50.0, 50.0]),
        ("wdg=2 kva=50 wdg=1 kva=75", [75.0, 75.0]),
        ("kvas=[75 50]", [75.0, 50.0]),
    ] {
        let text = format!(
            "new circuit.c\nnew transformer.t phases=1 buses=[sourcebus end] kvs=[12 .24] {rating}\n"
        );
        let module = powerio_dist::parse(source(&text)).unwrap();
        for (w, kva) in module.value().transformers()[0]
            .windings
            .iter()
            .zip(expected)
        {
            assert!((w.s_rating / 1000.0 - kva).abs() < 1e-12);
        }
    }
}

#[test]
fn retained_project_keeps_entrypoint_location_and_never_reacquires_profiles() {
    let dir = tempfile::tempdir().unwrap();
    let cases = dir.path().join("cases");
    std::fs::create_dir(&cases).unwrap();
    let data = dir.path().join("p.bin");
    let bytes = 7.0_f64.to_le_bytes();
    std::fs::write(&data, bytes).unwrap();
    let input = cases.join("master.dss");
    let text = "new circuit.c\nnew loadshape.s npts=1 mult=[dblfile=../p.bin] useactual=yes\nnew load.l bus1=sourcebus daily=s\n";
    std::fs::write(&input, text).unwrap();
    let module = powerio_dist::parse(
        Source::open(&input)
            .unwrap()
            .with_acquisition_root(dir.path())
            .unwrap(),
    )
    .unwrap();
    std::fs::remove_file(data).unwrap();
    let emitted = powerio_dist::emit(
        &module,
        DistTargetFormat::Dss,
        Destination::memory("out").unwrap(),
    )
    .unwrap();
    let EmittedOutput::Memory { artifacts } = emitted.into_output() else {
        panic!("memory output");
    };
    assert!(
        artifacts
            .iter()
            .any(|a| a.name().as_str() == "out/cases/master.dss" && a.bytes() == text.as_bytes())
    );
    assert!(
        artifacts
            .iter()
            .any(|a| a.name().as_str() == "out/p.bin" && a.bytes() == bytes)
    );
}

#[test]
fn absolute_dependencies_refuse_relocated_echo_but_canonical_output_works() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("p.bin");
    std::fs::write(&data, 7.0_f64.to_le_bytes()).unwrap();
    let input = dir.path().join("master.dss");
    std::fs::write(&input, format!("new circuit.c\nnew loadshape.s npts=1 mult=[dblfile={}] useactual=yes\nnew load.l bus1=sourcebus daily=s\n",data.display())).unwrap();
    let module = powerio_dist::parse(Source::open(input).unwrap()).unwrap();
    let error = powerio_dist::emit(
        &module,
        DistTargetFormat::Dss,
        Destination::memory("out").unwrap(),
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("EMIT.DSS.PROJECT_UNREPRESENTABLE")
    );
    assert!(
        powerio_dist::emit(
            &module.sever_source(),
            DistTargetFormat::Dss,
            Destination::memory("out").unwrap()
        )
        .is_ok()
    );
}

#[test]
fn clear_does_not_erase_the_project_dependency_portability_record() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("p.bin");
    std::fs::write(&data, 7.0_f64.to_le_bytes()).unwrap();
    let input = dir.path().join("master.dss");
    std::fs::write(&input, format!("new circuit.before\nnew loadshape.s npts=1 mult=[dblfile={}]\nclear\nnew circuit.after\n",data.display())).unwrap();
    let module = powerio_dist::parse(Source::open(input).unwrap()).unwrap();
    assert!(
        powerio_dist::emit(
            &module,
            DistTargetFormat::Dss,
            Destination::memory("out").unwrap()
        )
        .is_err()
    );
}

#[test]
fn native_single_precision_copy_does_not_activate_a_retained_double_q_array() {
    let bytes = [5.0_f32, 7.0, 3.0]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect::<Vec<_>>();
    for (edit, expected_q) in [("", 0.0), ("edit loadshape.t mult=[5 7 3]\n", 20.0)] {
        let text = format!(
            "new circuit.c\nnew loadshape.s npts=3 sngfile=p.bin useactual=yes\nnew loadshape.t npts=3 mult=[1 2 3] qmult=[10 20 30] useactual=yes like=s\n{edit}new load.l bus1=sourcebus daily=t\n"
        );
        let module = powerio_dist::parse(
            source(&text)
                .with_named_buffer("p.bin", bytes.clone())
                .unwrap(),
        )
        .unwrap();
        assert!(
            (module.value().loads()[0].q_nom.iter().sum::<f64>() / 1000.0 - expected_q).abs()
                < 1e-12
        );
    }
}

#[test]
fn batch_shape_edits_affect_future_load_assignments_only() {
    let text = "new circuit.c\nnew loadshape.s npts=3 mult=[5 7 3] qmult=[9 2 99] useactual=yes\nnew load.before bus1=sourcebus kw=4 kvar=1 daily=s\nbatchedit loadshape..* usea=no\nnew load.disabled bus1=sourcebus kw=4 kvar=1 daily=s\nbatchedit loadshape.^s$ npts=3 1 [10 20 30] qmult=[2 4 6] usea=yes\nnew load.after bus1=sourcebus kw=4 kvar=1 daily=s\n";
    let module = powerio_dist::parse(source(text)).unwrap();
    for (load, (p, q)) in module
        .value()
        .loads()
        .iter()
        .zip([(7.0, 2.0), (4.0, 1.0), (30.0, 6.0)])
    {
        assert!(
            (load.p_nom.iter().sum::<f64>() / 1000.0 - p).abs() < 1e-12,
            "{}",
            load.name
        );
        assert!(
            (load.q_nom.iter().sum::<f64>() / 1000.0 - q).abs() < 1e-12,
            "{}",
            load.name
        );
    }
}

#[test]
fn batch_shape_unsupported_actions_remain_explicit() {
    let text = "new circuit.c\nnew loadshape.s npts=3 mult=[5 7 3] useactual=yes\nbatchedit loadshape..* action=normalize\nnew load.l bus1=sourcebus daily=s\n";
    let error = powerio_dist::parse(source(text)).unwrap_err();
    assert!(
        error
            .diagnostics()
            .iter()
            .any(|d| d.code() == "READ.DSS.LOADSHAPE_UNRESOLVED")
    );
}
