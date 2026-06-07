//! File I/O: Matrix Market (`.mtx`) and JSON metadata.

pub mod meta;
pub mod mtx;

pub use meta::{CaseMetadata, MatrixMetadata, write_meta_json};
pub use mtx::{read_mtx, read_vector_mtx, write_mtx, write_vector_mtx};
