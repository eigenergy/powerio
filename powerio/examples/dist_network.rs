//! Validation bridge: distribution import, IR and independent canonical targets.
use powerio::{Destination, PioValue, Source};
use powerio_dist::{DistTargetFormat, audit_electrical_readiness};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let input = args
        .next()
        .ok_or("usage: dist_network INPUT OUTPUT_DIRECTORY [ACQUISITION_ROOT]")?;
    let output = std::path::PathBuf::from(args.next().ok_or("missing output directory")?);
    let mut source = Source::open(input)?;
    if let Some(root) = args.next() {
        source = source.with_acquisition_root(root)?;
    }
    if args.next().is_some() {
        return Err("unexpected argument".into());
    }
    std::fs::create_dir_all(&output)?;
    let module = powerio_dist::parse(source)?;
    let ir = output.join("network.pio.json");
    powerio::serialize(&module, ir.as_path())?;
    let restored = powerio::deserialize(ir.as_path())?;
    let PioValue::MulticonductorNetwork(network) = restored.value() else {
        return Err("IR changed structural type".into());
    };
    if serde_json::to_value(network)? != serde_json::to_value(module.value())? {
        return Err("IR changed network".into());
    }
    let mut targets = serde_json::Map::new();
    for (name, target) in [
        ("dss", DistTargetFormat::Dss),
        ("gridlabd", DistTargetFormat::Gridlabd),
    ] {
        // Fresh module deliberately excludes retained-source replay.
        let result = powerio_dist::emit(
            &powerio::PioModule::new(network.clone()),
            target,
            Destination::path(output.join(name)),
        );
        let value = match result {
            Ok(result) => {
                serde_json::json!({"ok": true, "diagnostics": diagnostics(result.diagnostics())})
            }
            Err(error) => serde_json::json!({"ok": false, "error": error.to_string()}),
        };
        targets.insert(name.into(), value);
    }
    let readiness = audit_electrical_readiness(network);
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "network": network,
            "ir_network_equal": true,
            "diagnostics": diagnostics(module.diagnostics()),
            "readiness": readiness.findings.iter().map(|f| serde_json::json!({"code": f.code, "element": f.element, "message": f.message, "severity": format!("{:?}",f.severity)})).collect::<Vec<_>>(),
            "targets": targets,
        }))?
    );
    Ok(())
}
fn diagnostics(items: &[powerio::Diagnostic]) -> Vec<serde_json::Value> {
    items
        .iter()
        .map(|d| serde_json::json!({"code":d.code(),"message":d.message()}))
        .collect()
}
