//! Explicit balanced-profile validation export, not automatic family detection.
use powerio_sincal::{DatabaseSnapshot, database_bytes};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: sincal_balanced <native SQLite or sinx>")?;
    let bytes = std::fs::read(&path)?;
    let database = database_bytes(&bytes)?;
    let snapshot = DatabaseSnapshot::decode(&database, None)?;
    let net = powerio_tx::format::__read_sincal_balanced_snapshot(&snapshot, &path)?;
    let report = serde_json::json!({
        "profile":"balanced", "schema":snapshot.version, "variant":snapshot.variant,
        "base_mva":net.base_mva(), "frequency":net.base_frequency(),
        "buses":net.buses(), "loads":net.loads(), "generators":net.generators(),
        "branches":net.branches(), "switches":net.switches(),
    });
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
