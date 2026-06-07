//! Bus adjacency matrix: `1` where an in-service branch joins two distinct
//! buses, `0` otherwise. Symmetric, zero diagonal, parallel branches
//! collapsed to a single `1`.

use std::collections::HashSet;

use sprs::CsMat;

use crate::indexed::IndexedNetwork;
use crate::matrix::triplet::CooBuilder;
use crate::{Error, Result};

/// Build the `n × n` 0/1 adjacency matrix.
pub fn build_adjacency(case: &IndexedNetwork) -> Result<CsMat<f64>> {
    let n = case.n();
    let mut edges: HashSet<(usize, usize)> = HashSet::new();
    for (idx, br) in case.in_service_branches() {
        let i = case
            .bus_index(br.from)
            .ok_or(Error::UnknownBus { bus_id: br.from, row: idx })?;
        let j = case
            .bus_index(br.to)
            .ok_or(Error::UnknownBus { bus_id: br.to, row: idx })?;
        if i != j {
            edges.insert(if i < j { (i, j) } else { (j, i) });
        }
    }

    let mut a = CooBuilder::with_capacity(n, 2 * edges.len());
    for (i, j) in edges {
        a.add(i, j, 1.0);
        a.add(j, i, 1.0);
    }
    Ok(a.finish_csr())
}
