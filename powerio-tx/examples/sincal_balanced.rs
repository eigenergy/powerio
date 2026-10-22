//! Explicit balanced-profile validation export, not automatic family detection.
use powerio_sincal::{DatabaseSnapshot, database_bytes};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let fresh_requested = args.iter().any(|a| a == "--fresh");
    let inputs = args
        .iter()
        .filter(|a| a.as_str() != "--fresh")
        .collect::<Vec<_>>();
    let path = inputs
        .first()
        .ok_or("usage: sincal_balanced <SQLite, sinx or MDB> [acquired.json] [hours] [--fresh]")?;
    if inputs.len() > 3 {
        return Err("too many arguments".into());
    }
    let bytes = std::fs::read(path)?;
    let hours = inputs.get(2).map(|s| s.parse::<f64>()).transpose()?;
    let snapshot = if let Some(records) = inputs.get(1) {
        let records = std::fs::read(records)?;
        powerio_sincal::TableRecords::decode(&records)?.verify_source(&bytes)?;
        DatabaseSnapshot::decode_records(&records, Some(1))?
    } else {
        let database = database_bytes(&bytes)?;
        DatabaseSnapshot::decode(&database, None)?
    };
    let net = powerio_tx::format::__read_sincal_balanced_snapshot_at(&snapshot, path, hours)?;
    let (net, fresh_diagnostics, schema, variant) = if fresh_requested {
        let output = powerio_tx::format::__write_sincal_balanced_experimental(&net)?;
        let fresh = DatabaseSnapshot::decode(&output.database, None)?;
        (
            powerio_tx::format::__read_sincal_balanced_snapshot(&fresh, path)?,
            output.diagnostics,
            fresh.version,
            fresh.variant,
        )
    } else {
        (net, Vec::new(), snapshot.version, snapshot.variant)
    };
    let report = serde_json::json!({
        "fresh_diagnostics":fresh_diagnostics,
        "profile":"balanced", "schema":schema, "variant":variant,
        "base_mva":net.base_mva(), "frequency":net.base_frequency(),
        "buses":net.buses(), "loads":net.loads(), "generators":net.generators(),
        "branches":net.branches(), "switches":net.switches(), "shunts":net.shunts(),
    });
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
