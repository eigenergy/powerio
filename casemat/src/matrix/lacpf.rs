//! Linear AC power flow (LACPF) block matrix.
//!
//! At flat start `u* = 1 + j0`, the linearized power flow Jacobian is
//!
//! ```text
//!   F(x*) = [[ G  -B  -I  0 ],
//!            [-B  -G   0 -I ]],
//! ```
//!
//! whose `2n × 2n` block (without the load injection identity columns) is
//!
//! ```text
//!   J = [[ G  -B ],
//!        [-B  -G ]].
//! ```
//!
//! Satisfies `p = G ε - B θ`, `q = -B ε - G θ`. Indefinite (saddle point);
//! emitted as a hard input alongside the SDDM B', B'', and ±Im(Y_bus).

use sprs::CsMat;

use crate::indexed::IndexedNetwork;
use crate::Result;

use super::ybus::build_ybus;
use super::BuildOptions;

pub fn build_lacpf(case: &IndexedNetwork, opts: &BuildOptions) -> Result<CsMat<f64>> {
    let parts = build_ybus(case, opts)?;
    let n = case.n();
    let two_n = 2 * n;

    // Walk both G and B once and emit the 4 blocks: [+G, -B; -B, -G].
    // `build_ybus` already returns CSR, so iterate the parts directly rather
    // than deep-copying through `to_csr()`.
    let mut tri = sprs::TriMat::with_capacity((two_n, two_n), 2 * (parts.g.nnz() + parts.b.nnz()));

    for (&v, (i, j)) in &parts.g {
        tri.add_triplet(i, j, v); // top-left:  +G
        tri.add_triplet(n + i, n + j, -v); // bottom-right: -G
    }
    for (&v, (i, j)) in &parts.b {
        tri.add_triplet(i, n + j, -v); // top-right:    -B
        tri.add_triplet(n + i, j, -v); // bottom-left:  -B
    }

    Ok(tri.to_csr())
}
