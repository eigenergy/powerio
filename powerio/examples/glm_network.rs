//! Validation bridge: GLM electrical import through the shared model and IR.
use powerio::{PioValue, Source};
use powerio_dist::glm::{GlmDialect, PreprocessOptions};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let input = args
        .next()
        .ok_or("usage: glm_network FILE IR_OUTPUT [pnnl|arras] [--dss-output FILE]")?;
    let output = args.next().ok_or("missing IR output path")?;
    let dialect = match args.next().as_deref() {
        None | Some("pnnl") => GlmDialect::Pnnl5_3,
        Some("arras") => GlmDialect::Arras4_3,
        _ => return Err("expected pnnl or arras dialect".into()),
    };
    let dss_output = match args.next().as_deref() {
        None => None,
        Some("--dss-output") => Some(args.next().ok_or("missing DSS output path")?),
        _ => return Err("unexpected argument".into()),
    };
    if args.next().is_some() {
        return Err("too many arguments".into());
    }
    let module = powerio_dist::glm::parse(
        Source::open(input)?,
        PreprocessOptions {
            dialect,
            ..Default::default()
        },
    )?;
    powerio::serialize(&module, output.as_str())?;
    let restored = powerio::deserialize(output.as_str())?;
    let PioValue::MulticonductorNetwork(network) = restored.value() else {
        return Err("IR changed the structural type".into());
    };
    if serde_json::to_value(network)? != serde_json::to_value(module.value())? {
        return Err("IR changed the electrical network".into());
    }
    let mut emission_codes = Vec::new();
    if let Some(path) = dss_output {
        let result = powerio_dist::emit(
            &powerio::PioModule::new(network.clone()),
            powerio_dist::DistTargetFormat::Dss,
            powerio::Destination::path(path),
        )?;
        emission_codes.extend(result.diagnostics().iter().map(|d| d.code().to_owned()));
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "network": network,
            "ir_network_equal": true,
            "dss_diagnostics": emission_codes,
            "diagnostics": module.diagnostics().iter().map(powerio::Diagnostic::code).collect::<Vec<_>>(),
        }))?
    );
    Ok(())
}
