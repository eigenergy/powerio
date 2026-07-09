//! File I/O: Matrix Market (`.mtx`) and JSON metadata, plus the gridfm-datakit
//! Parquet export (`--features gridfm`).

#[cfg(feature = "gridfm")]
pub mod gridfm;
pub mod meta;
pub mod mtx;
pub mod sensitivity;

#[cfg(feature = "gridfm")]
pub use gridfm::{
    GridfmOptions, GridfmOutputs, GridfmRead, GridfmSnapshot, GridfmTables, gridfm_base_case,
    gridfm_record_batches, gridfm_record_batches_batch, numbered_snapshots, read_gridfm_dataset,
    read_gridfm_network, read_gridfm_scenarios, write_gridfm_batch, write_gridfm_dataset,
};
pub use meta::{CaseMetadata, MatrixMetadata, write_meta_json};
pub use mtx::{read_mtx, read_vector_mtx, write_mtx, write_vector_mtx};
pub use sensitivity::write_sensitivity_mtx_with_options;

/// Read one scenario from a dataset directory in the named `from` format.
/// This function dispatches dataset format names; the C ABI's `pio_read_dir`
/// wraps it.
/// `gridfm` is the currently supported dataset format; `scenario` selects within it.
/// PyPSA CSV directories are case inputs, not datasets, and parse through
/// `parse_file`.
///
/// # Errors
/// [`powerio::Error::UnknownFormat`] for a non-dataset format name; otherwise
/// as [`read_gridfm_dataset`].
#[cfg(feature = "gridfm")]
pub fn read_dataset_dir(
    dir: impl AsRef<std::path::Path>,
    from: &str,
    scenario: i64,
) -> powerio::Result<GridfmRead> {
    require_dataset_format(from)?;
    read_gridfm_dataset(dir, scenario)
}

/// Return the distinct scenario IDs in ascending order for dataset directory
/// `dir` in the named `from` format. The C ABI exposes the same query through
/// `pio_scenario_ids`.
///
/// # Errors
/// As [`read_dataset_dir`].
#[cfg(feature = "gridfm")]
pub fn dataset_scenario_ids(
    dir: impl AsRef<std::path::Path>,
    from: &str,
) -> powerio::Result<Vec<i64>> {
    require_dataset_format(from)?;
    gridfm::gridfm_scenario_ids(dir)
}

#[cfg(feature = "gridfm")]
fn require_dataset_format(from: &str) -> powerio::Result<()> {
    if from.eq_ignore_ascii_case("gridfm") {
        return Ok(());
    }
    Err(powerio::Error::UnknownFormat(format!(
        "{from} is not a dataset directory format (dataset formats: gridfm); \
         PyPSA CSV directories parse through parse_file"
    )))
}
