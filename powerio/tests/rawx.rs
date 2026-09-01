use powerio::{Destination, EmittedOutput, FormatId, PioValue, Source, emit, parse};

const RAWX: &str = r#"{
  "network": {
    "caseid": {"fields":["rev","sbase","basfrq","title1"],"data":[35,100,60,"rawx-public"]},
    "bus": {"fields":["ibus","name","baskv","ide","area","zone","owner","vm","va"],"data":[[1,"Slack",230,3,1,1,1,1,0],[2,"Load",230,1,1,1,1,1,-1]]},
    "load": {"fields":["ibus","loadid","stat","pl","ql","ip","iq","yp","yq"],"data":[[2,"L1",1,20,5,1,2,3,4]]},
    "generator": {"fields":["ibus","machid","pg","qg","qt","qb","vs","ireg","nreg","mbase","stat","pt","pb"],"data":[[1,"G1",20,5,50,-50,1,2,0,100,1,80,0]]},
    "acline": {"fields":["ibus","jbus","ckt","rpu","xpu","bpu","rate1","stat"],"data":[[1,2,"1",0.01,0.1,0.02,100,1]]}
  }
}"#;

fn memory_text(result: powerio::EmitResult) -> String {
    let EmittedOutput::Memory { mut artifacts } = result.into_output() else {
        panic!("memory destination returned a path output");
    };
    String::from_utf8(artifacts.pop().unwrap().into_bytes()).unwrap()
}

#[test]
fn universal_parse_normalizes_rawx_metadata_and_echoes_exactly() {
    let source = Source::from_memory("input.json", RAWX.as_bytes().to_vec())
        .unwrap()
        .with_format(FormatId::new("rawx").unwrap());
    let module = parse(source, None).unwrap();
    assert_eq!(
        module.source().unwrap().format().unwrap().as_str(),
        "psse-rawx"
    );
    let PioValue::BalancedNetwork(network) = &module.value else {
        panic!("RAWX did not produce a balanced network");
    };
    assert!((network.loads()[0].p - 24.0).abs() < f64::EPSILON);
    assert!((network.loads()[0].q - 11.0).abs() < f64::EPSILON);

    let error = emit(&module, "rawx", Destination::memory("same.rawx").unwrap()).unwrap_err();
    assert_eq!(
        error.info().map(|info| info.code),
        Some("REQUEST.EMIT.UNKNOWN_FORMAT")
    );

    let result = emit(
        &module,
        "psse-rawx",
        Destination::memory("same.rawx").unwrap(),
    )
    .unwrap();
    assert_eq!(memory_text(result), RAWX);
}

#[test]
fn matpower_cross_format_output_reloads_as_rawx() {
    let matpower = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/case9.m");
    let module = parse(Source::open(matpower).unwrap(), None).unwrap();
    let result = emit(
        &module,
        "psse-rawx",
        Destination::memory("case9.rawx").unwrap(),
    )
    .unwrap();
    let text = memory_text(result);
    let root: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(root["network"]["caseid"]["data"][2], 35);

    let back = parse(
        Source::from_memory("case9.rawx", text.into_bytes()).unwrap(),
        None,
    )
    .unwrap();
    let PioValue::BalancedNetwork(network) = &back.value else {
        panic!("RAWX did not produce a balanced network");
    };
    assert_eq!(network.buses().len(), 9);
    assert_eq!(network.branches().len(), 9);
}
