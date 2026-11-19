//! Machine-readable units for the electrical fields of PowerIO IR.
//!
//! The annotations describe the stored, physical-unit model. They do not
//! describe normalized solver arrays. Base references are paths from the
//! containing network; `bus` and `from` resolve by source BusId.

use serde_json::{Value, json};

/// The stored sign of each signed electrical field.
const SIGN_CONVENTIONS: &[(&str, &str, &str)] = &[
    ("Generator", "pg", "positive injection into the network"),
    ("Generator", "qg", "positive injection into the network"),
    ("Load", "p", "positive consumption from the network"),
    ("Load", "q", "positive consumption from the network"),
    (
        "Storage",
        "ps",
        "positive charging consumption from the network",
    ),
    ("Storage", "qs", "positive consumption from the network"),
    ("Shunt", "g", "active consumption at unit voltage magnitude"),
    ("Shunt", "b", "reactive injection at unit voltage magnitude"),
    (
        "Branch",
        "b",
        "total charging susceptance; charging overrides its terminal split",
    ),
    ("Hvdc", "qf", "positive injection into the from bus"),
    ("Hvdc", "qt", "positive injection into the to bus"),
    (
        "HvdcConverter",
        "reactive_power_setpoint_mvar",
        "positive injection into the AC bus (PowSybl vscConverterStation)",
    ),
    (
        "VoltageSourceConverter",
        "reactive_power_setpoint_mvar",
        "positive withdrawal from the AC bus at the point of common coupling (load convention; PowSybl voltageSourceConverter, CGMES VsConverter.targetQpcc)",
    ),
    (
        "VoltageSourceConverter",
        "active_power_at_pcc_mw",
        "positive withdrawal from the AC bus at the point of common coupling (load convention)",
    ),
    (
        "VoltageSourceConverter",
        "reactive_power_at_pcc_mvar",
        "positive withdrawal from the AC bus at the point of common coupling (load convention)",
    ),
    (
        "VoltageSourceConverter",
        "target_active_power_mw",
        "positive withdrawal from the AC bus at the point of common coupling (load convention)",
    ),
];

pub(crate) fn annotate(schema: &mut schemars::Schema) {
    let Some(definitions) = schema
        .as_object_mut()
        .and_then(|o| o.get_mut("$defs"))
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    let groups: &[(&str, &str, &[&str])] = &[
        ("BalancedNetwork", "MVA", &["base_mva"]),
        ("BalancedNetwork", "Hz", &["base_frequency"]),
        ("Bus", "kV", &["base_kv"]),
        ("Bus", "deg", &["va"]),
        ("Bus", "pu", &["vm", "vmin", "vmax", "evhi", "evlo"]),
        ("Branch", "pu", &["r", "x", "b"]),
        ("Branch", "MVA", &["rate_a", "rate_b", "rate_c"]),
        ("Branch", "deg", &["shift", "angmin", "angmax"]),
        ("Branch", "1", &["tap"]),
        ("BranchCharging", "pu", &["g_fr", "b_fr", "g_to", "b_to"]),
        ("BranchSolution", "MW", &["pf", "pt"]),
        ("BranchSolution", "MVAr", &["qf", "qt"]),
        ("Generator", "MW", &["pg", "pmin", "pmax"]),
        ("Generator", "MVAr", &["qg", "qmin", "qmax"]),
        ("Generator", "MVA", &["mbase"]),
        ("Generator", "pu", &["vg"]),
        ("Load", "MW", &["p"]),
        ("Load", "MVAr", &["q"]),
        ("Shunt", "MW", &["g"]),
        ("Shunt", "MVAr", &["b"]),
        (
            "Storage",
            "MW",
            &["ps", "p_loss", "charge_rating", "discharge_rating"],
        ),
        ("Storage", "MVAr", &["qs", "q_loss", "qmin", "qmax"]),
        ("Storage", "MVA", &["thermal_rating"]),
        ("Storage", "MWh", &["energy", "energy_rating"]),
        (
            "Storage",
            "1",
            &["charge_efficiency", "discharge_efficiency"],
        ),
        ("Storage", "pu", &["r", "x"]),
    ];
    for &(definition, unit, fields) in groups {
        let properties = definitions
            .get_mut(definition)
            .and_then(|v| v.get_mut("properties"))
            .and_then(Value::as_object_mut)
            .expect("annotated electrical definition must exist");
        for field in fields {
            let property = properties
                .get_mut(*field)
                .and_then(Value::as_object_mut)
                .expect("annotated electrical field must exist");
            property.insert("x-unit".into(), json!(unit));
            if unit == "pu" {
                property.insert("x-unit-base".into(),match definition {"Bus"=>json!({"voltage":"base_kv"}),"Generator"|"Storage"=>json!({"power":"network.base_mva","voltage":"network.buses[bus].base_kv"}),_=>json!({"power":"network.base_mva","voltage":"network.buses[from].base_kv"})});
            }
        }
    }
    for &(kind, field, meaning) in SIGN_CONVENTIONS {
        definitions[kind]["properties"][field]["x-sign-convention"] = json!(meaning);
    }
    definitions["Branch"]["properties"]["tap"]["x-zero-means"] = json!(1.0);
    for (kind, fields) in [
        ("Branch", &["rate_a", "rate_b", "rate_c"][..]),
        ("Winding", &["rate_a", "rate_b", "rate_c"][..]),
        ("BranchRatingSet", &["rate_mva"][..]),
    ] {
        for field in fields {
            definitions[kind]["properties"][*field]["x-zero-means"] = json!("zero bound");
            definitions[kind]["properties"][*field]["x-infinity-means"] = json!("no bound");
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn physical_meaning_is_attached_to_the_stored_fields() {
        let schema = serde_json::to_value(crate::generate_ir_schema()).unwrap();
        assert_eq!(schema["$defs"]["Bus"]["properties"]["va"]["x-unit"], "deg");
        assert_eq!(
            schema["$defs"]["Branch"]["properties"]["r"]["x-unit-base"]["power"],
            "network.base_mva"
        );
        assert_eq!(
            schema["$defs"]["Storage"]["properties"]["energy"]["x-unit"],
            "MWh"
        );
        // The two converter setpoints state opposite conventions, as their
        // PowSybl sources do.
        assert!(
            schema["$defs"]["HvdcConverter"]["properties"]["reactive_power_setpoint_mvar"]
                ["x-sign-convention"]
                .as_str()
                .unwrap()
                .starts_with("positive injection")
        );
        assert!(
            schema["$defs"]["VoltageSourceConverter"]["properties"]["reactive_power_setpoint_mvar"]
                ["x-sign-convention"]
                .as_str()
                .unwrap()
                .starts_with("positive withdrawal")
        );
    }
}
