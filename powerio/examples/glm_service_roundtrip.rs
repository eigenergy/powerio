//! Native service oracle bridge: source-free fresh, IR, edited and repeated outputs.
use powerio::{Destination, PioValue, Source};
use powerio_dist::glm::{GlmDialect, PreprocessOptions, emit_canonical, parse};
use powerio_dist::{DistLoadVoltageModel, DistTargetFormat, MulticonductorNetwork};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let input = args.next().ok_or("expected INPUT OUTPUT pnnl|arras")?;
    let output = std::path::PathBuf::from(args.next().ok_or("missing output")?);
    let dialect = match args.next().as_deref() {
        Some("pnnl") => GlmDialect::Pnnl5_3,
        Some("arras") => GlmDialect::Arras4_3,
        _ => return Err("expected dialect".into()),
    };
    if args.next().is_some() {
        return Err("unexpected argument".into());
    }
    std::fs::create_dir_all(&output)?;
    let module = parse(
        Source::open(input)?,
        PreprocessOptions {
            dialect,
            ..Default::default()
        },
    )?;
    let ir = output.join("network.pio.json");
    powerio::serialize(&module, ir.as_path())?;
    let PioValue::MulticonductorNetwork(restored) =
        powerio::deserialize(ir.as_path())?.into_value()
    else {
        return Err("IR changed type".into());
    };
    let fresh = module.into_value();
    if serde_json::to_value(&fresh)? != serde_json::to_value(&restored)? {
        return Err("IR changed network".into());
    }
    let mut edited = fresh.clone();
    edit(&mut edited);
    let mut rebased = fresh.clone();
    for load in rebased.loads_mut() {
        let (v_nom, exponent) = match &mut load.voltage_model {
            DistLoadVoltageModel::ConstantCurrent { v_nom } => (v_nom, 1),
            DistLoadVoltageModel::ConstantImpedance { v_nom } => (v_nom, 2),
            _ => continue,
        };
        for (i, v) in v_nom.iter_mut().enumerate() {
            *v *= 0.7;
            load.p_nom[i] *= 0.7_f64.powi(exponent);
            load.q_nom[i] *= 0.7_f64.powi(exponent);
        }
    }
    let mut reports = serde_json::Map::new();
    for (label, net) in [
        ("fresh", fresh),
        ("restored", restored),
        ("edited", edited),
        ("rebased", rebased),
    ] {
        let folder = output.join(label);
        std::fs::create_dir_all(&folder)?;
        let path = folder.join("case.glm");
        let result = emit_canonical(&net, Destination::path(&path))?;
        powerio_dist::emit(
            &powerio::PioModule::new(net.clone()),
            DistTargetFormat::Dss,
            Destination::path(folder.join("dss")),
        )?;
        let reimported = parse(
            Source::open(&path)?,
            PreprocessOptions {
                dialect,
                ..Default::default()
            },
        )?;
        let reir = folder.join("reimported.pio.json");
        powerio::serialize(&reimported, reir.as_path())?;
        let PioValue::MulticonductorNetwork(repeated) =
            powerio::deserialize(reir.as_path())?.into_value()
        else {
            return Err("reimported IR type changed".into());
        };
        if serde_json::to_value(&repeated)? != serde_json::to_value(reimported.value())? {
            return Err("reimported IR changed network".into());
        }
        emit_canonical(&repeated, Destination::path(folder.join("repeated.glm")))?;
        powerio_dist::emit(
            &powerio::PioModule::new(repeated),
            DistTargetFormat::Dss,
            Destination::path(folder.join("repeated-dss")),
        )?;
        reports.insert(label.into(),serde_json::json!({"network":net,"ir_equal":true,"reimport_ir_equal":true,"diagnostics":result.diagnostics().iter().map(|d|serde_json::json!({"code":d.code(),"message":d.message()})).collect::<Vec<_>>()}));
    }
    println!("{}", serde_json::to_string_pretty(&reports)?);
    Ok(())
}
fn edit(net: &mut MulticonductorNetwork) {
    for load in net.loads_mut() {
        for p in &mut load.p_nom {
            *p *= 1.25;
        }
        for q in &mut load.q_nom {
            *q *= 1.25;
        }
    }
    for t in net.transformers_mut() {
        for w in &mut t.windings {
            w.r_pct *= 1.2;
        }
        let x = [t.xsc_pct[2] * 0.9, t.xsc_pct[4] * 0.9, t.xsc_pct[5] * 0.9];
        t.xsc_pct = vec![x[0] + x[1], x[0] + x[2], x[0], x[1] + x[2], x[1], x[2]];
        for w in &mut t.windings[1..3] {
            w.v_ref *= 1.05;
        }
        t.windings[0].tap *= 1.02;
        t.windings[3].tap *= 0.97;
    }
    for line in net.lines_mut() {
        line.length *= 1.1;
    }
    let f = net.base_frequency();
    for code in net
        .line_codes_mut()
        .iter_mut()
        .filter(|c| c.n_conductors == 3)
    {
        let earth = 0.001_588_36 * f / 1609.344;
        code.r_series[0][0] = earth + (code.r_series[0][0] - earth) * 1.17;
    }
    net.sources_mut()[0].v_magnitude[0] *= 1.03;
}
