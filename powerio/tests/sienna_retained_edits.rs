//! PowerIO IR retains the metadata needed for safe Sienna edit cycles.
use powerio::{Destination, EmittedOutput, PioModule, PioValue, Source};
use powerio_tx::network::BranchCharging;
use serde_json::Value;
use std::collections::BTreeSet;

fn bytes(result: powerio::EmitResult) -> Vec<u8> {
    let EmittedOutput::Memory { artifacts } = result.into_output() else {
        panic!("memory output")
    };
    artifacts[0].bytes().to_vec()
}

fn through_ir(module: &PioModule<PioValue>) -> PioModule<PioValue> {
    let ir =
        bytes(powerio::serialize(module, Destination::memory("case.pio.json").unwrap()).unwrap());
    powerio::deserialize(Source::from_memory("case.pio.json", ir).unwrap()).unwrap()
}

fn network(module: &PioModule<PioValue>) -> &powerio::BalancedNetwork {
    let PioValue::BalancedNetwork(net) = module.value() else {
        panic!("balanced network")
    };
    net
}

#[test]
fn cost_absence_and_secondary_charging_survive_ir_and_repeated_native_edits() {
    let source = Source::open(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/data/case9.m"),
    )
    .unwrap();
    let original = powerio::parse(source).unwrap();
    for target in ["sienna-0-4-json", "powersystems-json"] {
        let mut net = network(&original).clone();
        for g in net.generators_mut() {
            g.cost = None;
        }
        net.branches_mut()[0].tap = 1.02;
        net.branches_mut()[0].charging = Some(BranchCharging::new(0.0, 0.0, 0.001, 0.02));
        net.branches_mut()[0].b = 0.02;
        let mut module = PioModule::new(PioValue::BalancedNetwork(net));
        let mut identities = None;
        for _ in 0..3 {
            module = through_ir(&module);
            let PioValue::BalancedNetwork(net) = module.value_mut() else {
                panic!("balanced network")
            };
            net.buses_mut()[0].vm += 0.001;
            let out =
                powerio::emit(&module, target, Destination::memory("case.json").unwrap()).unwrap();
            assert!(
                out.diagnostics()
                    .iter()
                    .any(|d| d.message().contains("operation_cost defaults to zero"))
            );
            let content = bytes(out);
            if target == "powersystems-json" {
                let doc: Value = serde_json::from_slice(&content).unwrap();
                let current: BTreeSet<_> = doc["data"]["components"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|r| r["internal"]["uuid"]["value"].as_str().unwrap().to_owned())
                    .collect();
                if let Some(expected) = &identities {
                    assert_eq!(&current, expected);
                }
                identities = Some(current);
            }
            module = powerio::parse(Source::from_memory("case.json", content).unwrap()).unwrap();
            assert!(
                network(&module)
                    .generators()
                    .iter()
                    .all(|g| g.cost.is_none())
            );
            assert_eq!(network(&module).shunts().as_slice(), []);
            let charge = network(&module)
                .branches()
                .iter()
                .find(|b| b.is_transformer())
                .unwrap()
                .charging
                .unwrap();
            assert!((charge.g_to - 0.001).abs() < 1e-12);
            assert!((charge.b_to - 0.02).abs() < 1e-12);
        }
    }
}
