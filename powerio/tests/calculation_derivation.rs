use powerio::{HistoryKind, Source};

fn network_module() -> powerio::PioModule<powerio::PioValue> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../tests/data/case9.m");
    powerio::parse(Source::open(path).unwrap(), Some("matpower")).unwrap()
}

fn check_records<T>(module: &powerio::PioModule<T>, output_type: &str) {
    assert_eq!(module.producer().name(), "powerio");
    assert_eq!(module.producer().version(), powerio::VERSION);
    assert!(!module.sources().is_empty());
    assert!(module.source().is_none());
    assert!(module.source_map().is_empty());
    let history = module.history().last().unwrap();
    assert_eq!(history.kind(), HistoryKind::Transform);
    assert_eq!(history.input_type(), Some("powerio.BalancedNetwork"));
    assert_eq!(history.output_type(), Some(output_type));
}

#[test]
fn balanced_modules_derive_each_balanced_calculation_instance() {
    let source = network_module();
    assert!(source.source().is_some());

    let dc_pf = powerio::transform::to_dc_pf_instance(&source).unwrap();
    check_records(&dc_pf, "powerio.DcPfInstance");
    assert_eq!(dc_pf.value.network().buses().len(), 9);

    let ac_pf = powerio::transform::to_ac_pf_instance(&source).unwrap();
    check_records(&ac_pf, "powerio.AcPfInstance");
    assert_eq!(ac_pf.value.network().buses().len(), 9);

    let dc_opf = powerio::transform::to_dc_opf_instance(&source).unwrap();
    check_records(&dc_opf, "powerio.DcOpfInstance");
    assert_eq!(dc_opf.value.network().generators().len(), 3);

    let ac_opf = powerio::transform::to_ac_opf_instance(&source).unwrap();
    check_records(&ac_opf, "powerio.AcOpfInstance");
    assert_eq!(ac_opf.value.network().generators().len(), 3);

    assert!(source.source().is_some(), "the input module remains usable");
}

#[test]
fn calculation_derivation_requires_a_balanced_network_value() {
    let source = network_module();
    let instance = powerio::transform::to_dc_pf_instance(&source).unwrap();
    let dynamic = instance.map_value(powerio::PioValue::from);
    let error = powerio::transform::to_ac_pf_instance(&dynamic).unwrap_err();
    assert_eq!(
        error.info().unwrap().code,
        "REQUEST.MODULE.WRONG_MODEL_KIND"
    );
}

#[test]
fn an_already_typed_instance_is_extracted_without_reconstruction() {
    let source = network_module();

    let dc_pf = powerio::transform::to_dc_pf_instance(&source).unwrap();
    let history_len = dc_pf.history().len();
    let dynamic = dc_pf.map_value(powerio::PioValue::from);
    let extracted = powerio::transform::to_dc_pf_instance(&dynamic).unwrap();
    assert_eq!(extracted.history().len(), history_len);
    assert_eq!(extracted.value.network().buses().len(), 9);

    let ac_pf = powerio::transform::to_ac_pf_instance(&source).unwrap();
    let history_len = ac_pf.history().len();
    let dynamic = ac_pf.map_value(powerio::PioValue::from);
    let extracted = powerio::transform::to_ac_pf_instance(&dynamic).unwrap();
    assert_eq!(extracted.history().len(), history_len);

    let dc_opf = powerio::transform::to_dc_opf_instance(&source).unwrap();
    let history_len = dc_opf.history().len();
    let dynamic = dc_opf.map_value(powerio::PioValue::from);
    let extracted = powerio::transform::to_dc_opf_instance(&dynamic).unwrap();
    assert_eq!(extracted.history().len(), history_len);
    assert_eq!(extracted.value.network().generators().len(), 3);

    let ac_opf = powerio::transform::to_ac_opf_instance(&source).unwrap();
    let history_len = ac_opf.history().len();
    let dynamic = ac_opf.map_value(powerio::PioValue::from);
    let extracted = powerio::transform::to_ac_opf_instance(&dynamic).unwrap();
    assert_eq!(extracted.history().len(), history_len);
}

#[test]
fn an_already_typed_multiconductor_instance_is_extracted_without_reconstruction() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../tests/data/dist/bmopf/example_ieee13.json"
    );
    let source = powerio::parse(Source::open(path).unwrap(), Some("bmopf")).unwrap();

    let pf = powerio::transform::to_mc_ac_pf_instance(&source).unwrap();
    let history_len = pf.history().len();
    let dynamic = pf.map_value(powerio::PioValue::from);
    let extracted = powerio::transform::to_mc_ac_pf_instance(&dynamic).unwrap();
    assert_eq!(extracted.history().len(), history_len);

    let opf = powerio::transform::to_mc_ac_opf_instance(&source).unwrap();
    let history_len = opf.history().len();
    let dynamic = opf.map_value(powerio::PioValue::from);
    let extracted = powerio::transform::to_mc_ac_opf_instance(&dynamic).unwrap();
    assert_eq!(extracted.history().len(), history_len);
}
