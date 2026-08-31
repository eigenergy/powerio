//! File I/O: Matrix Market (`.mtx`) and JSON metadata, plus the gridfm-datakit
//! Parquet export (`--features gridfm`).

#[cfg(feature = "gridfm")]
pub mod gridfm;
pub mod meta;
pub mod mtx;
pub mod sensitivity;

#[cfg(feature = "gridfm")]
pub use gridfm::{
    GridfmOptions, GridfmOutputs, GridfmSnapshot, GridfmTables, emit_gridfm_batch,
    emit_gridfm_dataset, number_snapshots, to_gridfm_record_batches,
    to_gridfm_record_batches_single,
};
pub use meta::{CaseMetadata, MatrixMetadata, emit_meta_json};
pub use mtx::{
    emit_mtx, emit_vector_mtx, read_mtx, read_vector_mtx, to_mtx_bytes, to_vector_mtx_bytes,
};
pub use sensitivity::emit_sensitivity_mtx_with_options;
