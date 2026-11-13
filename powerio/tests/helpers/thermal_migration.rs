//! Check old numeric meanings at explicit network locations, including every
//! unrelated field in the comparison. Extras with the same names are controls.
use serde_json::{Value, json};

pub fn check(text: &str, network_paths: &[&str]) {
    let mut current: Value = serde_json::from_str(text).unwrap();
    let mut changed_paths = Vec::new();
    for network_path in network_paths {
        let network = current.pointer_mut(network_path).expect("network path");
        let branch = &mut network["branches"][0];
        assert!(branch.is_object(), "{network_path} needs a branch control");
        branch["rate_a"] = json!(0.0);
        branch["rate_b"] = json!(150.0);
        branch["rate_c"] = json!("Infinity");
        branch["rating_sets"] = json!([
            {"name":"zero", "rate_mva":0.0},
            {"name":"positive", "rate_mva":75.0},
            {"name":"infinite", "rate_mva":"Infinity"}
        ]);
        branch["extras"]["unrelated"] = json!({"rate_a":0.0, "rate_mva":0.0});
        changed_paths.push(format!("{network_path}/branches/0/rate_a"));
        changed_paths.push(format!("{network_path}/branches/0/rating_sets/0/rate_mva"));
    }
    let current_back = super::deserialize_module_text(&current.to_string()).unwrap();
    let current_back: Value =
        serde_json::from_str(&super::serialize_module_text(&current_back).unwrap()).unwrap();
    assert_eq!(current_back, current, "current zero limits changed");
    let mut old = current.clone();
    old["version"] = json!(2);
    let restored = super::deserialize_module_text(&old.to_string()).unwrap();
    let restored: Value =
        serde_json::from_str(&super::serialize_module_text(&restored).unwrap()).unwrap();
    for path in changed_paths {
        *current.pointer_mut(&path).unwrap() = json!("Infinity");
    }
    assert_eq!(restored, current, "legacy migration changed unrelated data");
}
