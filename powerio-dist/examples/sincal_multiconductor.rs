//! Explicit internal reader/audit harness; no automatic balancing fallback.
use powerio_sincal::{DatabaseSnapshot, database_bytes};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.len() < 3 || args.len() > 4 {
        return Err(
            "usage: sincal_multiconductor <native|records> <read|audit> <path> [variant]".into(),
        );
    }
    let bytes = std::fs::read(&args[2])?;
    let variant = args.get(3).map(|v| v.parse::<i64>()).transpose()?;
    let snapshot = match args[0].as_str() {
        "records" => DatabaseSnapshot::decode_records(&bytes, variant)?,
        "native" => DatabaseSnapshot::decode(&database_bytes(&bytes)?, variant)?,
        _ => return Err("expected explicit acquisition kind native or records".into()),
    };
    match args[1].as_str() {
        "read" => println!(
            "{}",
            serde_json::to_string_pretty(&powerio_dist::__read_sincal_multiconductor_snapshot(
                snapshot
            )?)?
        ),
        "audit" => println!(
            "{}",
            powerio_dist::__audit_sincal_multiconductor_snapshot(snapshot)?
        ),
        _ => return Err("expected read or audit".into()),
    }
    Ok(())
}
