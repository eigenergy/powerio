//! `CooBuilder` — a small HashMap backed coordinate format accumulator.
//! Deduplicates `(i, j)` entries on insert (each `add` is O(1) amortized,
//! independent of `nnz`). Replaces the previous Vec linear scan
//! accumulator, which was O(nnz²) per case.

use std::collections::HashMap;

use sprs::{CsMat, TriMat};

#[derive(Debug, Clone)]
pub struct CooBuilder {
    n: usize,
    entries: HashMap<(usize, usize), f64>,
}

impl CooBuilder {
    pub fn new(n: usize) -> Self {
        Self {
            n,
            entries: HashMap::new(),
        }
    }

    pub fn with_capacity(n: usize, capacity: usize) -> Self {
        Self {
            n,
            entries: HashMap::with_capacity(capacity),
        }
    }

    #[inline]
    pub fn n(&self) -> usize {
        self.n
    }

    /// Accumulate `v` into entry `(i, j)`. Skips the insert if `v == 0.0`.
    #[inline]
    pub fn add(&mut self, i: usize, j: usize, v: f64) {
        if v == 0.0 {
            return;
        }
        debug_assert!(i < self.n && j < self.n);
        *self.entries.entry((i, j)).or_insert(0.0) += v;
    }

    /// Symmetrically accumulate `v` into both `(i, j)` and `(j, i)`.
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
        let n = self.n;
        let mut tri = TriMat::with_capacity((n, n), self.entries.len());
        for ((i, j), v) in self.entries {
            if v != 0.0 {
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
