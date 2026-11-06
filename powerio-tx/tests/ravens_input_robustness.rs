use powerio_core::{FormatId, Source};
use serde_json::json;

#[test]
fn non_object_connectivity_nodes_return_parse_errors() {
    for value in [
        json!(null),
        json!(false),
        json!(1),
        json!("node"),
        json!([]),
    ] {
        let source = Source::from_memory(
            "case.json",
            serde_json::to_vec(&json!({"ConnectivityNode": {"1": value}})).unwrap(),
        )
        .unwrap()
        .with_format(FormatId::new("ravens-json").unwrap());
        let error = powerio_tx::parse(source).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("ConnectivityNode `1` is not an object"),
            "{error}"
        );
    }
}
