//! Native coordinate and unchanged-electrical-behavior validation bridge.
use powerio::{Destination, ParseOptions, PioModule, PioValue};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let input = args
        .next()
        .ok_or("usage: glm_locations FILE OUTPUT_DIRECTORY PROFILE")?;
    let output = std::path::PathBuf::from(args.next().ok_or("missing output directory")?);
    let profile = args.next().ok_or("missing profile")?;
    if args.next().is_some() {
        return Err("too many arguments".into());
    }
    let original =
        powerio::parse_with_options(input.as_str(), &ParseOptions::default().format(&profile)?)?;
    let ir = output.join("network.pio.json");
    powerio::serialize(&original, Destination::path(&ir))?;
    let restored = powerio::deserialize(ir.as_path())?;
    let PioValue::MulticonductorNetwork(net) = original.value() else {
        return Err("expected network".into());
    };
    let PioValue::MulticonductorNetwork(restored_net) = restored.value() else {
        return Err("expected restored network".into());
    };
    if serde_json::to_value(net)? != serde_json::to_value(restored_net)? {
        return Err("IR changed typed network or locations".into());
    }
    let fresh = PioModule::new(PioValue::from(net.clone()));
    let mut edited = fresh.clone();
    let PioValue::MulticonductorNetwork(edited_net) = edited.value_mut() else {
        unreachable!()
    };
    for location in edited_net.component_locations_mut() {
        if let Some(value) = &mut location.point.latitude {
            *value += 1.0;
        }
        if let Some(value) = &mut location.point.longitude {
            *value -= 1.0;
        }
    }
    let mut unlocated = net.clone();
    unlocated.component_locations_mut().clear();
    let unlocated = PioModule::new(PioValue::from(unlocated));
    let mut report = serde_json::Map::new();
    for (label, module) in [
        ("fresh", &fresh),
        ("restored", &restored),
        ("edited", &edited),
        ("unlocated", &unlocated),
    ] {
        let result = powerio::emit(module, &profile, Destination::path(output.join(label)))?;
        let PioValue::MulticonductorNetwork(net) = module.value() else {
            unreachable!()
        };
        let reparsed = powerio::parse_with_options(
            output.join(label).join("case.glm"),
            &ParseOptions::default().format(&profile)?,
        )?;
        let PioValue::MulticonductorNetwork(reparsed) = reparsed.value() else {
            unreachable!()
        };
        let mut electrical = net.clone();
        electrical.component_locations_mut().clear();
        report.insert(label.into(), serde_json::json!({
            "network": electrical,
            "locations": net.component_locations(),
            "reparsed_locations": reparsed.component_locations(),
            "diagnostics": result.diagnostics().iter().map(powerio::Diagnostic::code).collect::<Vec<_>>(),
        }));
    }
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
