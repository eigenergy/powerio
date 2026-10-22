//! Optional external-model evidence. Native LPC files are not redistributed.
use super::schema::NativeDatabase;
use crate::DistBus;

#[test]
#[ignore = "exports European LV line/load components; source and transformer remain unresolved"]
fn export_lpc_european_components() {
    let path = std::env::var_os("POWERIO_SINCAL_LPC_RECORDS").expect("external records path");
    let output = std::env::var_os("POWERIO_SINCAL_LPC_EXPORT").expect("explicit export path");
    let db = NativeDatabase::from_snapshot(
        powerio_sincal::DatabaseSnapshot::decode_records(&std::fs::read(path).unwrap(), Some(1))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(db.version.to_bits(), 12.8_f64.to_bits());
    let nodes = db.node_inputs().unwrap();
    let buses = nodes
        .keys()
        .map(|id| {
            (
                *id,
                DistBus::new(id.to_string(), ["1", "2", "3"].map(str::to_owned).to_vec()),
            )
        })
        .collect();
    let mut lines = Vec::new();
    let mut loads = Vec::new();
    for (&id, kind) in &db.elements {
        match kind.as_str() {
            "Line" => {
                let c = db.line_circuit(id, &buses).unwrap();
                lines.push(serde_json::json!({"element":id,"line":c.line,"code":c.code,
                    "auxiliary_buses":c.auxiliary_buses,"switches":c.terminal_switches}));
            }
            "Load" => {
                let input = db.load_input(id).unwrap();
                let node = &nodes[&input.terminal.node];
                let c = input
                    .circuit(&buses[&node.id], node.nominal_ll_volts)
                    .unwrap();
                loads.push(
                    serde_json::json!({"element":id,"load":c.load,"bus":c.bus,"switch":c.switch}),
                );
            }
            _ => {}
        }
    }
    assert_eq!((lines.len(), loads.len()), (205, 55));
    assert!(db.network().is_err());
    std::fs::write(
        output,
        serde_json::to_vec_pretty(&serde_json::json!({
            "scope":"components only; missing native source/transformer zero-sequence input",
            "lines":lines,"loads":loads
        }))
        .unwrap(),
    )
    .unwrap();
}
