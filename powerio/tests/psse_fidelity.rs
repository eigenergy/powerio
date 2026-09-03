use powerio::{
    Destination, EmittedOutput, FormatId, PioValue, Source, deserialize, emit, parse, serialize,
};

fn memory_text(result: powerio::EmitResult) -> String {
    let EmittedOutput::Memory { mut artifacts } = result.into_output() else {
        panic!("memory destination returned a path output");
    };
    String::from_utf8(artifacts.pop().unwrap().into_bytes()).unwrap()
}

fn raw_case(revision: u32) -> String {
    let generator = if revision >= 35 {
        "1, 'A', 20, 5, 50, -50, 1, 0, 0, 100, 0.0019, 0.29, 0.02, 0.31, 1.03, 1, 87.5, 80, 0, 3, 2, 0.75, 3, 0.25, 0, 1, 0, 1, 1, 0.97"
    } else {
        "1, 'A', 20, 5, 50, -50, 1, 0, 100, 0.0019, 0.29, 0.02, 0.31, 1.03, 1, 87.5, 80, 0, 2, 0.75, 3, 0.25, 0, 1, 0, 1, 1, 0.97"
    };
    format!(
        "0, 100, {revision}, 0, 0, 60 / synthetic\n\
         generator fidelity\n\
         \n\
         0 / END OF SYSTEM-WIDE DATA, BEGIN BUS DATA\n\
         1, 'BUS1', 230, 3, 1, 1, 1, 1, 0, 1.1, 0.9, 1.1, 0.9\n\
         0 / END OF BUS DATA, BEGIN LOAD DATA\n\
         0 / END OF LOAD DATA, BEGIN FIXED SHUNT DATA\n\
         0 / END OF FIXED SHUNT DATA, BEGIN GENERATOR DATA\n\
         {generator}\n\
         0 / END OF GENERATOR DATA, BEGIN BRANCH DATA\n\
         0 / END OF BRANCH DATA, BEGIN TRANSFORMER DATA\n\
         0 / END OF TRANSFORMER DATA, BEGIN AREA DATA\n\
         Q\n"
    )
}

fn generator_fields(text: &str) -> Vec<String> {
    text.lines()
        .find(|line| line.trim_start().starts_with("1, 'A',"))
        .expect("fresh output has the retained generator id")
        .split(',')
        .map(|field| field.trim().trim_matches('\'').to_owned())
        .collect()
}

#[test]
fn raw_generator_id_and_tail_survive_ir_and_fresh_output_for_revisions_33_to_35() {
    for (revision, format) in [(33, "psse"), (34, "psse34"), (35, "psse35")] {
        let source = Source::from_memory(
            format!("generator-{revision}.raw"),
            raw_case(revision).into_bytes(),
        )
        .unwrap()
        .with_format(FormatId::new(format).unwrap());
        let module = parse(source).unwrap();
        let PioValue::BalancedNetwork(network) = &module.value else {
            panic!("PSS/E RAW did not produce a balanced network");
        };
        let generator = &network.generators()[0];
        let metadata = network
            .detailed_connectivity()
            .as_deref()
            .unwrap()
            .component_metadata
            .iter()
            .find(|metadata| {
                metadata.component.component_type() == "generator"
                    && metadata.component.local_id() == generator.uid.as_deref().unwrap()
            })
            .unwrap();
        assert_eq!(metadata.properties["psse_eqid"], "A");
        assert_eq!(metadata.properties["psse_zx"], "0.29");

        let stored = serialize(
            &module,
            Destination::memory(format!("generator-{revision}.pio.json")).unwrap(),
        )
        .unwrap();
        let restored = deserialize(
            Source::from_memory(
                format!("generator-{revision}.pio.json"),
                memory_text(stored).into_bytes(),
            )
            .unwrap(),
        )
        .unwrap();
        if revision == 35 {
            let downgraded = emit(
                &restored,
                "psse",
                Destination::memory("fresh-33.raw").unwrap(),
            )
            .unwrap();
            assert!(downgraded.diagnostics().iter().any(|diagnostic| {
                diagnostic.code() == "EMIT.PSSE.FIELD_DROPPED"
                    && diagnostic.message().contains("BASLOD 3")
                    && diagnostic.message().contains("before revision 35")
            }));
        }
        let emitted = emit(
            &restored,
            format,
            Destination::memory(format!("fresh-{revision}.raw")).unwrap(),
        )
        .unwrap();
        let emitted = memory_text(emitted);
        let fields = generator_fields(&emitted);

        assert_eq!(fields[1], "A", "revision {revision}");
        let offset = usize::from(revision >= 35);
        assert_eq!(fields[9 + offset], "0.0019", "revision {revision}");
        assert_eq!(fields[10 + offset], "0.29", "revision {revision}");
        assert_eq!(fields[11 + offset], "0.02", "revision {revision}");
        assert_eq!(fields[12 + offset], "0.31", "revision {revision}");
        assert_eq!(fields[13 + offset], "1.03", "revision {revision}");
        assert_eq!(fields[15 + offset], "87.5", "revision {revision}");
        let owner_start = if revision >= 35 { 20 } else { 18 };
        if revision >= 35 {
            assert_eq!(fields[19], "3");
        }
        assert_eq!(fields[owner_start], "2", "revision {revision}");
        assert_eq!(fields[owner_start + 1], "0.75", "revision {revision}");
        assert_eq!(fields[owner_start + 2], "3", "revision {revision}");
        assert_eq!(fields[owner_start + 3], "0.25", "revision {revision}");
        assert_eq!(fields[owner_start + 8], "1", "revision {revision}");
        assert_eq!(fields[owner_start + 9], "0.97", "revision {revision}");

        let reparsed = parse(
            Source::from_memory(format!("fresh-{revision}.raw"), emitted.into_bytes())
                .unwrap()
                .with_format(FormatId::new(format).unwrap()),
        )
        .unwrap();
        let PioValue::BalancedNetwork(reparsed) = &reparsed.value() else {
            panic!("fresh PSS/E RAW did not produce a balanced network");
        };
        assert_eq!(reparsed.generators().len(), 1);
    }
}
