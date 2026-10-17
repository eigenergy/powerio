//! Structural inspection of explicitly acquired Access records. No electrical mapping.

use std::{fs::File, io::Read};

use powerio_sincal::{DatabaseSnapshot, MAX_BYTES};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args().skip(1);
    let path = arguments.next().ok_or("expected records.json [variant]")?;
    let variant = arguments.next().map(|v| v.parse()).transpose()?;
    if arguments.next().is_some() {
        return Err("expected records.json [variant]".into());
    }
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)?;
    let db = DatabaseSnapshot::decode_records(&bytes, variant)?;
    println!(
        "{}",
        serde_json::json!({
            "source_sha256":db.source_digest,
            "schema":db.version, "variant":db.variant,
            "nodes":db.nodes.len(), "elements":db.elements.len(),
            "terminals":db.terminals.len(), "excluded_tables":db.excluded_tables,
            "electrical_mapping_performed":false
        })
    );
    Ok(())
}
