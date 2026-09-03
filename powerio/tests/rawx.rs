use powerio::{
    Destination, EmittedOutput, FormatId, PioValue, Source, deserialize, emit, parse, serialize,
};

const RAWX: &str = r#"{
  "network": {
    "caseid": {"fields":["rev","sbase","basfrq","title1"],"data":[35,100,60,"rawx-public"]},
    "bus": {"fields":["ibus","name","baskv","ide","area","zone","owner","vm","va"],"data":[[1,"Slack",230,3,1,1,1,1,0],[2,"Load",230,1,1,1,1,1,-1]]},
    "load": {"fields":["ibus","loadid","stat","pl","ql","ip","iq","yp","yq"],"data":[[2,"L1",1,20,5,1,2,3,4]]},
    "generator": {"fields":["ibus","machid","pg","qg","qt","qb","vs","ireg","nreg","mbase","stat","pt","pb"],"data":[[1,"G1",20,5,50,-50,1,2,0,100,1,80,0]]},
    "acline": {"fields":["ibus","jbus","ckt","rpu","xpu","bpu","rate1","stat"],"data":[[1,2,"1",0.01,0.1,0.02,100,1]]}
  }
}"#;

const RAWX_FIDELITY: &str = r#"{
  "network": {
    "caseid": {"fields":["rev","sbase","basfrq","title1"],"data":[35,100,60,"rawx-fidelity"]},
    "bus": {"fields":["ibus","name","baskv","ide","area","zone","owner","vm","va"],"data":[[1,"Slack",230,3,1,1,1,1,0]]},
    "generator": {
      "fields":["ibus","machid","pg","qg","qt","qb","vs","ireg","nreg","mbase","zr","zx","rt","xt","gtap","stat","rmpct","pt","pb","baslod","o1","f1","o2","f2","o3","f3","o4","f4","wmod","wpf"],
      "data":[[1,"A",20,5,50,-50,1,0,0,100,0.0019,0.29,0.02,0.31,1.03,1,87.5,80,0,3,2,0.75,3,0.25,0,1,0,1,1,0.97]]
    },
    "swshunt": {
      "fields":["ibus","shntid","modsw","adjm","stat","vswhi","vswlo","swreg","nreg","rmpct","rmidnt","binit","s1","n1","b1"],
      "data":[[1,"S1",2,1,1,1.05,0.95,0,0,100,"",0,1,1,10]]
    },
    "sub": {"fields":["isub","name","lati","long","srg"],"data":[[1,"SUB",null,null,null]]},
    "subnode": {
      "fields":["isub","inode","name","ibus","stat","vm","va"],
      "data":[[1,1,"UNSET",1,1,null,null],[1,2,"ASSIGNED",1,1,0.98,-1.5]]
    }
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
    let module = parse(source).unwrap();
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
    let module = parse(Source::open(matpower).unwrap()).unwrap();
    let result = emit(
        &module,
        "psse-rawx",
        Destination::memory("case9.rawx").unwrap(),
    )
    .unwrap();
    let text = memory_text(result);
    let root: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(root["network"]["caseid"]["data"][2], 35);

    let back = parse(Source::from_memory("case9.rawx", text.into_bytes()).unwrap()).unwrap();
    let PioValue::BalancedNetwork(network) = &back.value else {
        panic!("RAWX did not produce a balanced network");
    };
    assert_eq!(network.buses().len(), 9);
    assert_eq!(network.branches().len(), 9);
}

#[test]
fn ir_fresh_emission_preserves_adjm_and_null_subnode_voltage() {
    let source = Source::from_memory("input.rawx", RAWX_FIDELITY.as_bytes().to_vec())
        .unwrap()
        .with_format(FormatId::new("psse-rawx").unwrap());
    let module = parse(source).unwrap();

    let stored = serialize(&module, Destination::memory("network.pio.json").unwrap()).unwrap();
    let stored = memory_text(stored);
    let restored =
        deserialize(Source::from_memory("network.pio.json", stored.into_bytes()).unwrap()).unwrap();
    let emitted = emit(
        &restored,
        "psse-rawx",
        Destination::memory("fresh.rawx").unwrap(),
    )
    .unwrap();
    let root: serde_json::Value = serde_json::from_str(&memory_text(emitted)).unwrap();

    let generator = &root["network"]["generator"];
    let generator_fields = generator["fields"].as_array().unwrap();
    let generator_row = generator["data"][0].as_array().unwrap();
    let generator_value = |field: &str| {
        let column = generator_fields
            .iter()
            .position(|candidate| candidate == field)
            .unwrap();
        &generator_row[column]
    };
    assert_eq!(generator_value("machid"), "A");
    assert_eq!(generator_value("zr"), 0.0019);
    assert_eq!(generator_value("zx"), 0.29);
    assert_eq!(generator_value("rt"), 0.02);
    assert_eq!(generator_value("xt"), 0.31);
    assert_eq!(generator_value("gtap"), 1.03);
    assert_eq!(generator_value("rmpct"), 87.5);
    assert_eq!(generator_value("baslod"), 3);
    assert_eq!(generator_value("o1"), 2);
    assert_eq!(generator_value("f1"), 0.75);
    assert_eq!(generator_value("o2"), 3);
    assert_eq!(generator_value("f2"), 0.25);
    assert_eq!(generator_value("wmod"), 1);
    assert_eq!(generator_value("wpf"), 0.97);
    assert_eq!(root["network"]["swshunt"]["data"][0][3], 1);
    assert!(root["network"]["subnode"]["data"][0][5].is_null());
    assert!(root["network"]["subnode"]["data"][0][6].is_null());
    assert_eq!(root["network"]["subnode"]["data"][1][5], 0.98);
    assert_eq!(root["network"]["subnode"]["data"][1][6], -1.5);
}
