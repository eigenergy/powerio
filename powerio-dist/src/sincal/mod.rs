//! Private conductor-resolved SINCAL adapter and acquisition staging.
//!
//! Internal until conductor semantics and the electrical mapper are complete.
//! Successful schema decoding does not imply electrical-model support.
//! SINCAL also carries balanced profiles, whose mapper belongs in powerio-tx.
//! This module must not become the facade's unconditional SINCAL route or a
//! dependency of powerio-tx. Archive acquisition and retention are shared via
//! powerio-sincal, together with bounded schema/identity validation. Electrical
//! interpretation remains on this family-local owner; public dispatch must
//! select the appropriate adapter explicitly.

mod acquisition;
mod connection;
#[cfg(test)]
mod connection_tests;
mod dc_infeeder;
#[cfg(test)]
mod dc_infeeder_tests;
mod grounding;
mod infeeder;
#[cfg(test)]
mod legacy_tests;
mod line_mapping;
mod load;
mod load_mapping;
mod load_profile;
#[cfg(test)]
mod load_profile_tests;
mod mapping;
mod mapping_report;
mod project;
#[cfg(test)]
mod project_tests;
mod provenance;
mod schema;
mod semantics;
mod sequence;
mod shunt_impedance;
#[cfg(test)]
mod shunt_impedance_tests;
mod source_mapping;
#[cfg(test)]
mod source_mapping_tests;
mod topology;
mod transformer;
mod transformer_impedance;
mod transformer_mapping;

use crate::{Error, Result};
use powerio_core::Source;

use schema::NativeDatabase;

fn read(source: &Source, variant: Option<i64>) -> Result<NativeDatabase> {
    let primary = source.primary_buffer().map_err(format_error)?;
    let bytes = acquisition::database_bytes(primary.bytes())?;
    NativeDatabase::decode(&bytes, variant)
}

fn format_error(message: impl std::fmt::Display) -> Error {
    Error::FormatRead {
        format: "PSS SINCAL",
        message: message.to_string(),
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod transformer_tests;

#[cfg(test)]
mod grounding_tests;

#[cfg(test)]
mod topology_tests;

#[cfg(test)]
mod infeeder_tests;

#[cfg(test)]
mod load_mapping_tests;

#[cfg(test)]
mod line_mapping_tests;

#[cfg(test)]
mod transformer_mapping_tests;

#[cfg(test)]
mod mapping_tests;

#[cfg(test)]
mod mapping_report_tests;

pub(crate) fn read_snapshot(
    snapshot: powerio_sincal::DatabaseSnapshot,
) -> Result<crate::MulticonductorNetwork> {
    NativeDatabase::from_snapshot(snapshot)?.network()
}

pub(crate) fn audit_snapshot(snapshot: powerio_sincal::DatabaseSnapshot) -> Result<String> {
    let report = NativeDatabase::from_snapshot(snapshot)?.mapping_report()?;
    serde_json::to_string_pretty(&report).map_err(format_error)
}

pub(crate) fn read_snapshot_at(
    snapshot: powerio_sincal::DatabaseSnapshot,
    hours: f64,
) -> Result<crate::MulticonductorNetwork> {
    NativeDatabase::from_snapshot(snapshot)?.network_at(hours)
}

pub(crate) fn audit_snapshot_at(
    snapshot: powerio_sincal::DatabaseSnapshot,
    hours: f64,
) -> Result<String> {
    let report = NativeDatabase::from_snapshot(snapshot)?.mapping_report_at(Some(hours))?;
    serde_json::to_string_pretty(&report).map_err(format_error)
}
