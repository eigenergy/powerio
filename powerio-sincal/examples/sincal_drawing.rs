//! Drawing-only validation, independent of electrical adapter support.
use powerio_sincal::{DatabaseSnapshot, MAX_BYTES};
use std::{fs::File, io::Read};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("expected acquired-records.json")?;
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)?;
    let db = DatabaseSnapshot::decode_records(&bytes, Some(1))?;
    println!(
        "{}",
        serde_json::json!({"geometry":db.drawing_geometry()?,"schema":db.version,
        "source_sha256":db.source_digest,"electrical_mapping_performed":false})
    );
    Ok(())
}
