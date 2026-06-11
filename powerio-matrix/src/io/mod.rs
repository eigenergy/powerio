//! File I/O: Matrix Market (`.mtx`) and JSON metadata, plus the gridfm-datakit
//! Parquet export (`--features gridfm`).

#[cfg(feature = "gridfm")]
pub mod gridfm;
pub mod meta;
pub mod mtx;

#[cfg(feature = "gridfm")]
pub use gridfm::{
    GridfmOptions, GridfmOutputs, GridfmRead, GridfmSnapshot, GridfmTables, gridfm_base_case,
    gridfm_record_batches, gridfm_record_batches_batch, numbered_snapshots, read_gridfm_dataset,
    read_gridfm_network, read_gridfm_scenarios, write_gridfm_batch, write_gridfm_dataset,
};
pub use meta::{CaseMetadata, MatrixMetadata, write_meta_json};
pub use mtx::{read_mtx, read_vector_mtx, write_mtx, write_vector_mtx};
