//! Explicit balanced-profile validation export, not automatic family detection.
use powerio_sincal::{DatabaseSnapshot, database_bytes};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: sincal_balanced <native SQLite, sinx or MDB> [acquired.json] [hours]")?;
    let bytes = std::fs::read(&path)?;
    let record_path = std::env::args().nth(2);
    let hours = std::env::args()
        .nth(3)
        .map(|s| s.parse::<f64>())
        .transpose()?;
    let snapshot = if let Some(records) = record_path {
        let records = std::fs::read(records)?;
        powerio_sincal::TableRecords::decode(&records)?.verify_source(&bytes)?;
        DatabaseSnapshot::decode_records(&records, Some(1))?
    } else {
        let database = database_bytes(&bytes)?;
        DatabaseSnapshot::decode(&database, None)?
    };
    let net = powerio_tx::format::__read_sincal_balanced_snapshot_at(&snapshot, &path, hours)?;
    let report = serde_json::json!({
        "profile":"balanced", "schema":snapshot.version, "variant":snapshot.variant,
        "base_mva":net.base_mva(), "frequency":net.base_frequency(),
        "buses":net.buses(), "loads":net.loads(), "generators":net.generators(),
        "branches":net.branches(), "switches":net.switches(), "shunts":net.shunts(),
    });
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
