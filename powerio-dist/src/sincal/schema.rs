//! Family-local ownership and error adaptation for shared schema validation.

use std::ops::Deref;

use powerio_sincal::DatabaseSnapshot;
use rusqlite::Connection;

use super::format_error;
use crate::Result;

// Keep electrical interpretation as inherent methods of this local owner.
// The shared snapshot supplies identities and the bounded query-only database.
pub(super) struct NativeDatabase(DatabaseSnapshot);

impl NativeDatabase {
    pub fn from_snapshot(snapshot: DatabaseSnapshot) -> Result<Self> {
        // Structural admission of modern balanced schemas does not authorize
        // their conductor semantics here. Keep family support independent.
        if ![11.5_f64, 14.8]
            .iter()
            .any(|v| v.to_bits() == snapshot.version.to_bits())
        {
            return Err(format_error(format!(
                "unsupported multiconductor electrical schema {}",
                snapshot.version
            )));
        }
        Ok(Self(snapshot))
    }

    pub fn decode(bytes: &[u8], requested_variant: Option<i64>) -> Result<Self> {
        Self::from_snapshot(
            DatabaseSnapshot::decode(bytes, requested_variant).map_err(format_error)?,
        )
    }
}

impl Deref for NativeDatabase {
    type Target = DatabaseSnapshot;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub(super) fn require_table(connection: &Connection, table: &str, required: &[&str]) -> Result<()> {
    powerio_sincal::require_table(connection, table, required).map_err(format_error)
}

#[cfg(test)]
pub(super) fn verify_read_only(bytes: &[u8]) {
    let database = NativeDatabase::decode(bytes, None).unwrap();
    assert!(database.connection.execute("DELETE FROM Node", []).is_err());
    assert!(
        database
            .connection
            .execute("ATTACH DATABASE ':memory:' AS other", [])
            .is_err()
    );
}
