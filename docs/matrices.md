# Matrix outputs and conventions

The `powerio-matrix` crate exposes efficient sparse matrix and graph views for common power system representations. The views are derived from a parsed `Network`. The builders take the densely indexed `IndexedNetwork`, which maps bus ids to a
contiguous `[0, n)`. 

**Note:** The experimental DC OPF bundle currently has its own schema in [dcopf-bundle.md](dcopf-bundle.md), and per-builder API
detail is in the [crate docs](https://eigenergy.github.io/powerio/powerio_matrix/).

## Current capabilities

| matrix | shape | builder | notes |
| --- | --- | --- | --- |
| B' (FDPF) | n×n | `build_bprime` | singular positive Laplacian, rank n−1, shuntless |
| B'' (FDPF) | n×n | `build_bdoubleprime` | SDDM when bus shunts are present |
| `Re(Y_bus)`, `-Im(Y_bus)` | n×n | `build_ybus` | full admittance, keeps taps and shifts |
| LACPF block | 2n×2n | `build_lacpf` | `[[G, −B], [−B, −G]]`, flat start, indefinite |
| signed incidence `A` | n×m | `build_incidence` | column `e`: `+1` at from-bus, `−1` at to-bus |
| weighted Laplacian `L` | n×n | `build_weighted_laplacian` | `L = A diag(w) Aᵀ`, `ground_at` removes a row/col |
| flow map `B Aᵀ` | m×n | `build_flow_map` | `f = B Aᵀ θ` |
| PTDF | m×n | `build_ptdf` | dense; factors the Laplacian grounded at the reference buses |
| LODF | m×m | `build_lodf` | dense DC line-outage factors |
| adjacency | n×n | `build_adjacency` | `MatrixKind::Adjacency` |
| petgraph view | n/a | `IndexedNetwork::to_petgraph` | `UnGraph<bus_idx, branch_idx>` |

Computing PTDF and LODF matrices requires a linear solve, which is not the focus of powerio. Both factor the Laplacian with one row and column removed for each reference bus, using the dense Cholesky in
`matrix::sensitivity`. Every connected component must contain at least one
reference bus. PTDF is dense `m×n`; sparse work would mean selected column
computation or sparse factorization, not a sparse PTDF matrix. The DC OPF
instance bundle (`A`, `b`, `L`, costs, bounds, thermal limits, `C_g`) is documented in
[dcopf-bundle.md](dcopf-bundle.md).

## Conventions

- **Positive Laplacian matrices.** Off-diagonal `< 0`, diagonal `> 0`, with `diag = Σ|off-diag|`
  for B' susceptance matrices. This is the M-matrix form an SDDM or Cholesky solver expects; a consumer can recover an edge weight as `−L[i,j] > 0`.
- **Bus indexing.** Bus ids are 1-based and preserved on the model, refer to the Rust [New Type Idiom](https://doc.rust-lang.org/rust-by-example/generics/new_types.html).
  `IndexedNetwork::bus_index(id)` is the only mapping into the dense `[0, n)`; an id
  out of range is an `Error::UnknownBus`.
- **Taps and shifts.** `tap == 0` means `tap = 1` (`Branch::effective_tap`). B'
  ignores taps and shifts; B'' keeps taps and zeros only shifts; Y_bus keeps both.
- **`BR_B` is already per unit.** Line charging susceptance is per unit on `baseMVA`
  in MATPOWER; never divide by `base_mva` again.
- **Zero impedance branches.** B' skips them by default
  (`BuildOptions::skip_zero_impedance`; set it `false` to get
  `Error::ZeroImpedance`). Y_bus scatters no admittance for them (`r² + x² = 0`)
  and the incidence builder drops them, both unconditionally. The gridfm export
  counts the drops (`dropped_zero_impedance` in `gridfm_meta.json`); surfacing a
  drop count on the matrix builders is tracked in #50.
- **Reference coverage.** `IndexedNetwork::check_reference_coverage` verifies that
  every in-service island has a reference bus.
- **Susceptance conventions for the DC approximation.** The default is `b = 1/x` (`DcConvention::PaperPure`, taps and
  shifts ignored), which makes `L = A diag(b) Aᵀ` equal `build_bprime` in the XB
  scheme. `DcConvention::Matpower` uses `b = 1/(x·τ)` plus a phase shift injection
  `p_shift`.

## Output

Matrices write as Matrix Market or stay in memory. A symmetric matrix writes the
lower triangle with the `symmetric` header, 1-based indices, via `io::mtx::write_mtx`
(sprs writes the upper triangle, so powerio ships its own spec-compliant writer).
The `sensitivities` and `dcopf` CLI subcommands bundle the relevant family with a
JSON manifest.

The petgraph view (`UnGraph<usize, usize>`, node weight = dense bus index, edge
weight = branch index) backs the connectivity and radial diagnostics and is the
substrate for spanning-tree work (LinDist3Flow).
