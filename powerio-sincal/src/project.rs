use powerio_core::{Source, SourceBuffer};

use crate::{
    MAX_BYTES, Result, SQLITE_MAGIC,
    acquisition::{read_bounded, validated_archive},
    format_error,
};

/// Retained archive namespace and database bytes, without electrical meaning.
pub struct AcquiredProject {
    pub source: Source,
    pub database: SourceBuffer,
}

impl AcquiredProject {
    pub fn read(source: &Source) -> Result<Self> {
        let primary = source.primary_buffer().map_err(format_error)?;
        if primary.bytes().len() as u64 > MAX_BYTES {
            return Err(format_error("input exceeds 64 MiB limit"));
        }
        if primary.bytes().starts_with(SQLITE_MAGIC) {
            return Ok(Self {
                source: source.clone(),
                database: primary,
            });
        }
        let (mut archive, index) = validated_archive(primary.bytes())?;
        let database_name = archive
            .by_index(index)
            .map_err(format_error)?
            .name()
            .to_owned();
        // Packages have their own namespace. Caller or disk companions must
        // never fill a missing archive reference.
        let mut retained =
            Source::from_memory(source.name(), primary.shared_bytes()).map_err(format_error)?;
        let mut actual_expanded = 0_u64;
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).map_err(format_error)?;
            if entry.is_dir() {
                continue;
            }
            let name = entry.name().to_owned();
            let bytes = read_bounded(&mut entry)?;
            actual_expanded = actual_expanded
                .checked_add(bytes.len() as u64)
                .ok_or_else(|| format_error("archive expanded byte limit exceeded"))?;
            if actual_expanded > MAX_BYTES {
                return Err(format_error("archive expanded byte limit exceeded"));
            }
            retained = retained
                .with_named_buffer(name, bytes)
                .map_err(format_error)?;
        }
        let database = retained.root_buffer(&database_name).map_err(format_error)?;
        Ok(Self {
            source: retained,
            database,
        })
    }
}
