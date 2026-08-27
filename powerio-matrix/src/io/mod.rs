//! File I/O: Matrix Market (`.mtx`) and JSON metadata, plus the gridfm-datakit
//! Parquet export (`--features gridfm`).

#[cfg(feature = "gridfm")]
pub mod gridfm;
pub mod meta;
pub mod mtx;
pub mod sensitivity;

#[cfg(feature = "gridfm")]
pub use gridfm::{
    GridfmOptions, GridfmOutputs, GridfmSnapshot, GridfmTables, gridfm_record_batches,
    gridfm_record_batches_single, numbered_snapshots, write_gridfm_batch, write_gridfm_dataset,
};
pub use meta::{CaseMetadata, MatrixMetadata, write_meta_json};
pub use mtx::{
    mtx_bytes, read_mtx, read_vector_mtx, vector_mtx_bytes, write_mtx, write_vector_mtx,
};
pub use sensitivity::write_sensitivity_mtx_with_options;
