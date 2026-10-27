//! Validation bridge for facade profile selection and project/canonical output.
use powerio::{Destination, ParseOptions, PioValue};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let input = args
        .next()
        .ok_or("usage: glm_dispatch FILE OUTPUT_DIRECTORY PROFILE")?;
    let output = std::path::PathBuf::from(args.next().ok_or("missing output directory")?);
    let profile = args.next().ok_or("missing profile")?;
    if !matches!(profile.as_str(), "gridlabd" | "gridlabd-arras") || args.next().is_some() {
        return Err("expected gridlabd or gridlabd-arras profile".into());
    }
    let original =
        powerio::parse_with_options(input.as_str(), &ParseOptions::default().format(&profile)?)?;
    let ir = output.join("network.pio.json");
    powerio::serialize(&original, Destination::path(&ir))?;
    let restored = powerio::deserialize(ir.as_path())?;
    let mut edited = restored.clone();
    let PioValue::MulticonductorNetwork(net) = edited.value_mut() else {
        return Err("expected network".into());
    };
    for load in net.loads_mut() {
        for p in &mut load.p_nom {
            *p *= 1.25;
        }
        for q in &mut load.q_nom {
            *q *= 1.25;
        }
    }
    let mut report = serde_json::Map::new();
    for (label, module, target) in [
        ("replay", &original, profile.as_str()),
        ("canonical", &restored, profile.as_str()),
        ("edited", &edited, profile.as_str()),
        (
            "cross_profile",
            &original,
            if profile == "gridlabd" {
                "gridlabd-arras"
            } else {
                "gridlabd"
            },
        ),
    ] {
        let result = powerio::emit(module, target, Destination::path(output.join(label)))?;
        let PioValue::MulticonductorNetwork(net) = module.value() else {
            return Err("expected network".into());
        };
        report.insert(label.into(), serde_json::json!({
            "network": net,
            "target": target,
            "fidelity": format!("{:?}", result.fidelity()),
            "diagnostics": result.diagnostics().iter().map(powerio::Diagnostic::code).collect::<Vec<_>>(),
        }));
    }
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
