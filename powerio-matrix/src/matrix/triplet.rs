//! `CooBuilder` — a small HashMap backed coordinate format accumulator.
//! Deduplicates `(i, j)` entries on insert (each `add` is O(1) amortized,
//! independent of `nnz`). Replaces the previous Vec linear scan
//! accumulator, which was O(nnz²) per case.
//!
//! Square by default (`new`), rectangular via `new_rect` for the incidence,
//! flow map, and generator→bus matrices.

use rustc_hash::{FxBuildHasher, FxHashMap};
use sprs::{CsMat, TriMat};

type CoordinateMap = FxHashMap<usize, f64>;

#[derive(Debug, Clone)]
pub struct CooBuilder {
    rows: usize,
    cols: usize,
    entries: CoordinateMap,
}

impl CooBuilder {
    /// Square `n × n` accumulator.
    pub fn new(n: usize) -> Self {
        Self::new_rect(n, n)
    }

    /// Square `n × n` accumulator with a pre-sized entry table.
    pub fn with_capacity(n: usize, capacity: usize) -> Self {
        Self::with_capacity_rect(n, n, capacity)
    }

    /// Rectangular `rows × cols` accumulator.
    pub fn new_rect(rows: usize, cols: usize) -> Self {
        Self {
            rows,
            cols,
            entries: CoordinateMap::default(),
        }
    }

    /// Rectangular `rows × cols` accumulator with a pre-sized entry table.
    pub fn with_capacity_rect(rows: usize, cols: usize, capacity: usize) -> Self {
        Self {
            rows,
            cols,
            entries: CoordinateMap::with_capacity_and_hasher(capacity, FxBuildHasher),
        }
    }

    /// Side length for a square builder (row count in general).
    #[inline]
    pub fn n(&self) -> usize {
        self.rows
    }

    /// `(rows, cols)`.
    #[inline]
    pub fn shape(&self) -> (usize, usize) {
        (self.rows, self.cols)
    }

    /// Accumulate `v` into entry `(i, j)`. Skips the insert if `v == 0.0`.
    ///
    /// # Panics
    /// Panics if `(i, j)` is outside the matrix shape or the packed coordinate
    /// key overflows `usize`.
    #[inline]
    pub fn add(&mut self, i: usize, j: usize, v: f64) {
        if v == 0.0 {
            return;
        }
        assert!(
            i < self.rows && j < self.cols,
            "COO coordinate ({i}, {j}) out of bounds for shape {}x{}",
            self.rows,
            self.cols
        );
        let key = i
            .checked_mul(self.cols)
            .and_then(|base| base.checked_add(j))
            .expect("COO matrix dimensions overflow usize");
        *self.entries.entry(key).or_insert(0.0) += v;
    }

    /// Symmetrically accumulate `v` into both `(i, j)` and `(j, i)`. Square
    /// builders only.
    #[inline]
    pub fn add_sym(&mut self, i: usize, j: usize, v: f64) {
        if i == j {
            self.add(i, j, v);
        } else {
            self.add(i, j, v);
            self.add(j, i, v);
        }
    }

    /// Materialize as a `CsMat<f64>` (CSR) with explicit zeros pruned.
    pub fn finish_csr(self) -> CsMat<f64> {
        let mut tri = TriMat::with_capacity((self.rows, self.cols), self.entries.len());
        for (key, v) in self.entries {
            if v != 0.0 {
                let i = key / self.cols;
                let j = key % self.cols;
                tri.add_triplet(i, j, v);
            }
        }
        tri.to_csr()
    }

    /// Materialize as a CSC matrix.
    pub fn finish_csc(self) -> CsMat<f64> {
        self.finish_csr().to_csc()
    }
}
