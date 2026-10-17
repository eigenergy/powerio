//! Validation-only fresh, IR-restored and edited canonical GLM output.
use powerio::{Destination, PioValue, Source};
use powerio_dist::glm::{GlmDialect, PreprocessOptions, emit_canonical, parse};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let input = args
        .next()
        .ok_or("usage: glm_roundtrip FILE OUTPUT_DIRECTORY [pnnl|arras]")?;
    let output = std::path::PathBuf::from(args.next().ok_or("missing output directory")?);
    let dialect = match args.next().as_deref() {
        None | Some("pnnl") => GlmDialect::Pnnl5_3,
        Some("arras") => GlmDialect::Arras4_3,
        _ => return Err("expected pnnl or arras dialect".into()),
    };
    if args.next().is_some() {
        return Err("too many arguments".into());
    }
    let module = parse(
        Source::open(input)?,
        PreprocessOptions {
            dialect,
            ..Default::default()
        },
    )?;
    let ir_path = output.join("network.pio.json");
    powerio::serialize(&module, ir_path.as_path())?;
    let restored = powerio::deserialize(ir_path.as_path())?;
    let PioValue::MulticonductorNetwork(restored) = restored.into_value() else {
        return Err("IR changed type".into());
    };
    let fresh = module.into_value();
    if serde_json::to_value(&fresh)? != serde_json::to_value(&restored)? {
        return Err("IR changed network".into());
    }
    let mut edited = fresh.clone();
    for load in edited.loads_mut() {
        for p in &mut load.p_nom {
            *p *= 1.25;
        }
        for q in &mut load.q_nom {
            *q *= 1.25;
        }
    }
    for line in edited.lines_mut() {
        line.length *= 1.1;
    }
    let source = edited
        .sources_mut()
        .first_mut()
        .ok_or("oracle needs a fixed source")?;
    *source
        .v_magnitude
        .first_mut()
        .ok_or("oracle source needs a phase")? *= 1.03;
    let mut rebased = fresh.clone();
    for load in rebased.loads_mut() {
        use powerio_dist::DistLoadVoltageModel;
        let (v_nom, exponent) = match &mut load.voltage_model {
            DistLoadVoltageModel::ConstantCurrent { v_nom } => (v_nom, 1),
            DistLoadVoltageModel::ConstantImpedance { v_nom } => (v_nom, 2),
            _ => continue,
        };
        for (i, nominal) in v_nom.iter_mut().enumerate() {
            let scale: f64 = if i % 2 == 0 { 0.5 } else { 2.0 };
            *nominal *= scale;
            load.p_nom[i] *= scale.powi(exponent);
            load.q_nom[i] *= scale.powi(exponent);
        }
    }
    let mut referenced = fresh.clone();
    for source in referenced.sources_mut() {
        source.reference_terminal = Some("n".into());
    }
    let mut reports = serde_json::Map::new();
    for (label, network) in [
        ("fresh", fresh),
        ("restored", restored),
        ("edited", edited),
        ("rebased", rebased),
        ("grounded_reference", referenced),
    ] {
        let path = output.join(format!("{label}.glm"));
        let emission = emit_canonical(&network, Destination::path(path))?;
        reports.insert(label.into(), serde_json::json!({
            "network": network,
            "diagnostics": emission.diagnostics().iter().map(powerio::Diagnostic::code).collect::<Vec<_>>(),
        }));
    }
    println!("{}", serde_json::to_string_pretty(&reports)?);
    Ok(())
}
