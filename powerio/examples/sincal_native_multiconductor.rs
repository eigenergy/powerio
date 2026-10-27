//! Native SQLite -> public distribution reader -> retained source, IR and generic consumers.
use powerio::{Destination, EmittedOutput, ParseOptions, PioValue, Source};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("expected native SQLite path")?;
    let source = Source::open(path)?;
    let original = source.primary_buffer()?;
    let module = powerio::parse_with_options(
        source,
        &ParseOptions::default().format("sincal-multiconductor")?,
    )?;
    let EmittedOutput::Memory { artifacts } = powerio::emit(
        &module,
        "sincal-multiconductor",
        Destination::memory("echo.db")?,
    )?
    .into_output() else {
        return Err("expected memory output".into());
    };
    if artifacts.len() != 1 || artifacts[0].bytes() != original.bytes() {
        return Err("native source echo differs".into());
    }
    let EmittedOutput::Memory { mut artifacts } =
        powerio::serialize(&module, Destination::memory("network.pio.json")?)?.into_output()
    else {
        return Err("expected IR output".into());
    };
    let restored = powerio::deserialize(Source::from_memory(
        "network.pio.json",
        artifacts.remove(0).into_bytes(),
    )?)?;
    let (PioValue::MulticonductorNetwork(network), PioValue::MulticonductorNetwork(copy)) =
        (module.value(), restored.value())
    else {
        return Err("wrong network family".into());
    };
    if serde_json::to_value(network)? != serde_json::to_value(copy)? {
        return Err("IR value differs".into());
    }
    let matrix =
        powerio_matrix::matrix::multiconductor::calc_multiconductor_admittance_matrix(network)?;
    if !matrix.diagnostics().is_empty() {
        return Err("generic matrix omitted components".into());
    }
    powerio::to_mc_ac_pf_instance(&module)?;
    if powerio::emit(
        &restored,
        "sincal-multiconductor",
        Destination::memory("invalid.db")?,
    )
    .is_ok()
    {
        return Err("restored IR emitted retained native source".into());
    }
    eprintln!(
        "{}",
        serde_json::json!({"source_echo":true,"typed_ir":true,"generic_matrix_diagnostics":0,"power_flow_instance":true})
    );
    println!("{}", serde_json::to_string(network)?);
    Ok(())
}
