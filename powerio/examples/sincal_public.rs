//! Explicit Access acquisition -> public facade -> IR and source-echo check.
//! Usage: sincal_public original.mdb acquired-records.json snapshot-hours
use powerio::{Destination, EmittedOutput, ParseOptions, PioValue, Source};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if !(args.len() == 3 || (args.len() == 4 && args[3] == "--assume-inactive-source-controls")) {
        return Err("expected original.mdb acquired-records.json snapshot-hours [--assume-inactive-source-controls]".into());
    }
    let native = Source::open(&args[0])?.primary_buffer()?;
    let records = Source::open(&args[1])?.primary_buffer()?;
    let source = Source::from_memory("original.mdb", native.shared_bytes())?
        .with_named_buffer("acquired.json", records.shared_bytes())?;
    let mut selection = powerio_dist::SincalReadOptions::default();
    selection.assume_inactive_source_controls = args.len() == 4;
    selection.variant = Some(1);
    selection.snapshot_hours = Some(args[2].parse()?);
    selection.acquired_tables = Some("acquired.json".into());
    let mut options = ParseOptions::default().format("sincal-multiconductor")?;
    options.sincal_multiconductor = Some(selection);
    let module = powerio::parse_with_options(source, &options)?;
    let echo = powerio::emit(
        &module,
        "sincal-multiconductor",
        Destination::memory("echo.mdb")?,
    )?;
    let EmittedOutput::Memory { artifacts } = echo.into_output() else {
        return Err("expected memory echo".into());
    };
    if artifacts.len() != 1 || artifacts[0].bytes() != native.bytes() {
        return Err("original MDB echo differs".into());
    }
    let serialized = powerio::serialize(&module, Destination::memory("case.pio.json")?)?;
    let EmittedOutput::Memory { mut artifacts } = serialized.into_output() else {
        return Err("expected IR".into());
    };
    let restored = powerio::deserialize(Source::from_memory(
        "case.pio.json",
        artifacts.remove(0).into_bytes(),
    )?)?;
    let (PioValue::MulticonductorNetwork(a), PioValue::MulticonductorNetwork(b)) =
        (module.value(), restored.value())
    else {
        return Err("IR family changed".into());
    };
    if serde_json::to_value(a)? != serde_json::to_value(b)? {
        return Err("IR value differs".into());
    }
    if powerio::emit(&restored, "sincal", Destination::memory("invalid.mdb")?).is_ok() {
        return Err("IR restoration echoed native bytes".into());
    }
    let PioValue::MulticonductorNetwork(network) = module.value() else {
        return Err("wrong family".into());
    };
    let admittance =
        powerio_matrix::matrix::multiconductor::calc_multiconductor_admittance_matrix(network)?;
    if !admittance.diagnostics().is_empty() {
        return Err(format!(
            "incomplete generic matrix assembly: {:?}",
            admittance.diagnostics()
        )
        .into());
    }
    // Preserve the parsed network even when the generic calculation boundary
    // rejects de-energized islands. The external checker examines this outcome.
    let pf = match powerio::to_mc_ac_pf_instance(&module) {
        Ok(_) => serde_json::json!({"constructed": true}),
        Err(error) => serde_json::json!({
            "constructed": false,
            "diagnostics": error.diagnostics().iter().map(|d| {
                serde_json::json!({"code": d.code(), "message": d.message()})
            }).collect::<Vec<_>>()
        }),
    };
    eprintln!(
        "{}",
        serde_json::json!({
            "reader_diagnostics": module.diagnostics().iter().map(|d| serde_json::json!({"code": d.code(), "message": d.message()})).collect::<Vec<_>>(),
            "generic_matrix_diagnostics": admittance.diagnostics().len(),
            "power_flow_instance": pf,
        })
    );
    println!("{}", serde_json::to_string(network)?);
    Ok(())
}
