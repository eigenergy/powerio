//! External balanced Access validation through the public facade, including echo and IR.
use powerio::{Destination, EmittedOutput, ParseOptions, PioValue, Source};

fn bytes(result: powerio::EmitResult) -> Vec<u8> {
    let EmittedOutput::Memory { mut artifacts } = result.into_output() else {
        panic!("expected memory output")
    };
    assert_eq!(artifacts.len(), 1);
    artifacts.remove(0).into_bytes()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 3 {
        return Err("usage: sincal_balanced_access <original.mdb> <acquired.json> <hours>".into());
    }
    let original = std::fs::read(&args[0])?;
    let records = std::fs::read(&args[1])?;
    let source = Source::from_memory("original.mdb", original.clone())?
        .with_named_buffer("acquired.json", records.clone())?;
    let mut selection = powerio_tx::format::SincalBalancedReadOptions::default();
    selection.variant = Some(1);
    selection.snapshot_hours = Some(args[2].parse()?);
    selection.acquired_tables = Some("acquired.json".into());
    let mut options = ParseOptions::default().format("sincal-balanced")?;
    options.sincal_balanced = Some(selection);
    let module = powerio::parse_with_options(source.clone(), &options)?;
    let PioValue::BalancedNetwork(net) = module.value() else {
        panic!("explicit balanced family must remain balanced")
    };
    let echoed = bytes(powerio::emit(
        &module,
        "sincal-balanced",
        Destination::memory("copy.mdb")?,
    )?);
    assert_eq!(echoed, original);
    let ir = bytes(powerio::serialize(
        &module,
        Destination::memory("case.pio.json")?,
    )?);
    let restored = powerio::deserialize(Source::from_memory("case.pio.json", ir)?)?;
    let PioValue::BalancedNetwork(restored_net) = restored.value() else {
        panic!("IR must retain the balanced family")
    };
    assert_eq!(
        serde_json::to_value(restored_net)?,
        serde_json::to_value(net)?
    );
    assert!(
        powerio::emit(
            &restored,
            "sincal-balanced",
            Destination::memory("stale.mdb")?
        )
        .is_err()
    );
    let mut wrong = options.clone().format("sincal-multiconductor")?;
    assert!(powerio::parse_with_options(source.clone(), &wrong).is_err());
    wrong = options.clone();
    wrong.sincal_balanced.as_mut().unwrap().snapshot_hours = None;
    assert!(powerio::parse_with_options(source.clone(), &wrong).is_err());
    wrong = options.clone();
    wrong.sincal_balanced.as_mut().unwrap().variant = Some(999_999);
    assert!(powerio::parse_with_options(source, &wrong).is_err());
    let mut corrupted = original;
    *corrupted.last_mut().unwrap() ^= 1;
    let corrupted = Source::from_memory("original.mdb", corrupted)?
        .with_named_buffer("acquired.json", records.clone())?;
    assert!(powerio::parse_with_options(corrupted, &options).is_err());
    let doc: serde_json::Value = serde_json::from_slice(&records)?;
    let version = doc["tables"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "Version")
        .unwrap();
    let column = version["columns"]
        .as_array()
        .unwrap()
        .iter()
        .position(|c| c["name"] == "Version_No")
        .unwrap();
    let report = serde_json::json!({
        "profile":"balanced", "schema":version["rows"][0][column], "variant":1,
        "public_facade_access_selection":true, "source_echo_and_ir_checked":true,
        "wrong_family_missing_time_variant_and_origin_rejected":true,
        "base_mva":net.base_mva(), "frequency":net.base_frequency(),
        "buses":net.buses(), "loads":net.loads(), "generators":net.generators(),
        "branches":net.branches(), "switches":net.switches(), "shunts":net.shunts(),
    });
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
