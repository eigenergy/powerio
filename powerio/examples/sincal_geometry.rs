//! Public SINCAL geometry validation probe. See evals/sincal/verify_geometry.py.
use powerio::{Destination, EmittedOutput, ParseOptions, PioValue, Source};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() < 2 || args.len() > 4 {
        return Err(
            "expected FAMILY NATIVE [ACQUIRED_RECORDS] [--assume-inactive-source-controls]".into(),
        );
    }
    let mut source = Source::open(&args[1])?;
    let native = source.primary_buffer()?;
    let records = args.get(2).filter(|s| !s.starts_with("--"));
    if let Some(path) = records {
        source = Source::from_memory("original.mdb", native.shared_bytes())?.with_named_buffer(
            "acquired.json",
            Source::open(path)?.primary_buffer()?.shared_bytes(),
        )?;
    }
    let mut options = ParseOptions::default().format(&args[0])?;
    if args[0] == "sincal-balanced" {
        let mut selection = powerio_tx::format::SincalBalancedReadOptions::default();
        selection.variant = Some(1);
        selection.snapshot_hours = records.map(|_| 0.0);
        selection.acquired_tables = records.map(|_| "acquired.json".into());
        options.sincal_balanced = Some(selection);
    } else if args[0] == "sincal-multiconductor" {
        let mut selection = powerio_dist::SincalReadOptions::default();
        selection.variant = Some(1);
        selection.snapshot_hours = records.map(|_| 0.0);
        selection.acquired_tables = records.map(|_| "acquired.json".into());
        selection.assume_inactive_source_controls = args
            .iter()
            .any(|s| s == "--assume-inactive-source-controls");
        options.sincal_multiconductor = Some(selection);
    } else {
        return Err("expected explicit SINCAL family".into());
    }
    let module = powerio::parse_with_options(source, &options)?;
    let EmittedOutput::Memory { artifacts } =
        powerio::emit(&module, "sincal", Destination::memory("echo")?)?.into_output()
    else {
        return Err("expected memory".into());
    };
    if artifacts.len() != 1 || artifacts[0].bytes() != native.bytes() {
        return Err("source echo differs".into());
    }
    let EmittedOutput::Memory { mut artifacts } =
        powerio::serialize(&module, Destination::memory("case.pio.json")?)?.into_output()
    else {
        return Err("expected memory".into());
    };
    let restored = powerio::deserialize(Source::from_memory(
        "case.pio.json",
        artifacts.remove(0).into_bytes(),
    )?)?;
    let (network, copy, layer) = match (module.value(), restored.value()) {
        (PioValue::BalancedNetwork(a), PioValue::BalancedNetwork(b)) => (
            serde_json::to_value(a)?,
            serde_json::to_value(b)?,
            a.to_geo_layer(),
        ),
        (PioValue::MulticonductorNetwork(a), PioValue::MulticonductorNetwork(b)) => (
            serde_json::to_value(a)?,
            serde_json::to_value(b)?,
            powerio::dist_geo::to_dist_geo_layer(a),
        ),
        _ => return Err("wrong network family".into()),
    };
    if network != copy {
        return Err("IR value differs".into());
    }
    if !layer.features.is_empty() {
        // GeoLayer parsing intentionally trims display names, including MDB padding.
        // Native IDs, coordinates, CRS and source/derived kinds must remain exact.
        let mut canonical = layer.clone();
        for feature in &mut canonical.features {
            feature.key.name = feature.key.name.as_ref().map(|s| s.trim().to_owned());
        }
        if powerio::GeoLayer::parse(&layer.to_geojson(), None)?.layer != canonical {
            return Err("geometry export differs".into());
        }
    }
    println!(
        "{}",
        serde_json::json!({"network":network,"layer":layer,"source_echo":true,"typed_ir":true,"geo_export":!layer.features.is_empty(),"diagnostics":module.diagnostics()})
    );
    Ok(())
}
