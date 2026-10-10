//! Validation-only fresh, IR-restored and edited canonical GLM output.
use powerio::{Destination, PioValue, Source};
use powerio_dist::glm::{GlmDialect, PreprocessOptions, emit_canonical, parse};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let input = args
        .next()
        .ok_or("usage: glm_roundtrip FILE OUTPUT_DIRECTORY [pnnl|arras] [LINE_SHUNTS_JSON] [--acquisition-root DIRECTORY]")?;
    let output = std::path::PathBuf::from(args.next().ok_or("missing output directory")?);
    let dialect = match args.next().as_deref() {
        None | Some("pnnl") => GlmDialect::Pnnl5_3,
        Some("arras") => GlmDialect::Arras4_3,
        _ => return Err("expected pnnl or arras dialect".into()),
    };
    let (source, shunts) = source_options(&mut args, Source::open(input)?)?;
    let mut module = parse(
        source,
        PreprocessOptions {
            dialect,
            ..Default::default()
        },
    )?;
    if let Some(path) = shunts {
        override_shunts(&mut module, &path)?;
    }

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
    edit_transformers(&mut edited);
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
        if !network.transformers().is_empty() {
            powerio_dist::emit(
                &powerio::PioModule::new(network.clone()),
                powerio_dist::DistTargetFormat::Dss,
                Destination::path(output.join(format!("{label}.dss"))),
            )?;
        }
        reports.insert(label.into(), serde_json::json!({
            "network": network,
            "diagnostics": emission.diagnostics().iter().map(powerio::Diagnostic::code).collect::<Vec<_>>(),
        }));
    }
    println!("{}", serde_json::to_string_pretty(&reports)?);
    Ok(())
}

fn source_options(
    args: &mut impl Iterator<Item = String>,
    mut source: Source,
) -> Result<(Source, Option<String>), Box<dyn std::error::Error>> {
    let mut shunts = None;
    while let Some(arg) = args.next() {
        if arg == "--acquisition-root" {
            source =
                source.with_acquisition_root(args.next().ok_or("missing acquisition root")?)?;
        } else if shunts.is_none() && !arg.starts_with("--") {
            shunts = Some(arg);
        } else {
            return Err("unexpected argument".into());
        }
    }
    Ok((source, shunts))
}

fn override_shunts(
    module: &mut powerio::PioModule<powerio_dist::MulticonductorNetwork>,
    path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct EndShunts {
        g_from: Vec<Vec<f64>>,
        b_from: Vec<Vec<f64>>,
        g_to: Vec<Vec<f64>>,
        b_to: Vec<Vec<f64>>,
    }
    let shunts: EndShunts = serde_json::from_slice(&std::fs::read(path)?)?;
    let [code] = module.value_mut().line_codes_mut().as_mut_slice() else {
        return Err("shunt oracle override requires exactly one linecode".into());
    };
    code.g_from = shunts.g_from;
    code.b_from = shunts.b_from;
    code.g_to = shunts.g_to;
    code.b_to = shunts.b_to;
    Ok(())
}

fn edit_transformers(network: &mut powerio_dist::MulticonductorNetwork) {
    for transformer in network.transformers_mut() {
        if transformer.xsc_pct == [0.0] {
            transformer.windings[0].tap *= 1.02;
        } else if transformer.phases == 1
            && transformer.windings.len() == 2
            && transformer.windings[0].terminal_map.len() == 2
            && transformer.windings[0]
                .terminal_map
                .iter()
                .all(|t| matches!(t.as_str(), "1" | "2" | "3"))
        {
            // Exercise current tap bases and leakage split on phase-pair banks.
            transformer.windings[0].tap *= 1.02;
            transformer.windings[1].tap *= 1.01;
            transformer.windings[0].r_pct = transformer.windings[1].r_pct / 3.0;
        }
        for winding in &mut transformer.windings {
            winding.r_pct *= 1.2;
        }
        if transformer.windings.len() == 4 && transformer.xsc_pct.len() == 6 {
            let x = [
                transformer.xsc_pct[2] * 0.9,
                transformer.xsc_pct[4] * 0.9,
                transformer.xsc_pct[5] * 0.9,
            ];
            transformer.xsc_pct = vec![x[0] + x[1], x[0] + x[2], x[0], x[1] + x[2], x[1], x[2]];
            for winding in &mut transformer.windings[1..3] {
                winding.v_ref *= 1.05;
            }
            transformer.windings[0].tap *= 1.02;
            transformer.windings[3].tap *= 0.97;
            continue;
        }
        for x in &mut transformer.xsc_pct {
            *x *= 0.9;
        }
        if let Some(secondary) = transformer.windings.get_mut(1) {
            secondary.v_ref *= 1.05;
        }
    }
}
